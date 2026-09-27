# UI/UX Part 03 — Android Jetpack Compose App Shell, Navigation & Lifecycle Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 03  
**Platform:** Android  
**UI framework:** Pure Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary goal:** define the Android application shell, Compose navigation, adaptive phone/tablet/foldable layouts, lifecycle ownership, process-death recovery, Rust/JNI presentation bridge, permissions, system intents, notifications, foreground services, incoming calls, Picture-in-Picture, accessibility, and state-restoration architecture.

---

# 1. Purpose

The Android application must feel like a first-class Android application rather than a desktop or cross-platform UI forced onto a phone.

Android imposes platform-specific constraints and expectations around:

```text
Activity lifecycle
process death
background execution
notifications
permissions
predictive back
IME
system share sheet
file picker
camera
microphone
Bluetooth
NFC
foreground services
Picture-in-Picture
window size classes
foldables
accessibility
```

Jetpack Compose should own these interaction mechanics while the Rust runtime continues to own:

```text
messaging truth
security truth
call state
file-transfer state
presence
search
backup
sync
identity
routing
```

The governing principle is:

> **Compose owns Android presentation and platform interaction; Rust owns the product state and business rules.**

---

# 2. Architectural Position

```text
                     Rust Core
                        │
              Rust Presentation Service
                        │
                     JNI Bridge
                        │
                 Kotlin Repository
                        │
                    ViewModel
                        │
                    StateFlow
                        │
                 Jetpack Compose
                        │
              Android Platform APIs
```

---

# 3. Android Module Layout

Recommended:

```text
apps/android/
├── app/
│   └── src/main/
│       ├── kotlin/
│       │   ├── app/
│       │   ├── navigation/
│       │   ├── ui/
│       │   ├── feature/
│       │   ├── platform/
│       │   ├── rust/
│       │   └── service/
│       ├── res/
│       └── AndroidManifest.xml
└── build.gradle.kts
```

---

# 4. Kotlin Package Structure

Suggested:

```text
app/
    App.kt
    MainActivity.kt

navigation/
    AppDestination.kt
    AppNavHost.kt
    DeepLinkRouter.kt

ui/
    theme/
    common/
    adaptive/

feature/
    chats/
    calls/
    contacts/
    files/
    search/
    devices/
    security/
    settings/

platform/
    permissions/
    notifications/
    share/
    picker/
    biometric/
    bluetooth/
    nfc/
    pip/

rust/
    RustBridge.kt
    RustRepository.kt
    RustEventAdapter.kt

service/
    CallForegroundService.kt
    BackgroundWakeService.kt
```

---

# 5. Single Source of Truth

Compose must never create a second independent domain model.

Correct:

```text
Rust State
  ↓
Kotlin presentation mapping
  ↓
Compose
```

Incorrect:

```text
Rust messages
+
separate Kotlin message DB
+
ViewModel business rules
```

---

# 6. Kotlin ViewModel Role

ViewModel owns:

```text
screen UI state
navigation-adjacent state
temporary input state
permission flow state
loading/error presentation
StateFlow lifecycle
```

ViewModel must not own:

```text
message delivery semantics
security decisions
call state machine
transfer truth
sync conflict resolution
```

---

# 7. Rust Bridge

The JNI boundary should expose semantic operations.

Examples:

```text
listConversations()
openConversation(id)
sendMessage(command)
observeConversation(id)
startCall(peer)
acceptCall(callId)
search(query)
startTransfer(fileHandle)
```

---

# 8. Avoid Fine-Grained JNI

Do not cross JNI for every:

```text
message field
pixel
audio frame
video frame
database row
```

Use:

```text
coarse DTOs
batched updates
handles
event streams
```

---

# 9. Event Bridge

Recommended:

```text
Rust bounded event channel
        ↓
JNI event dispatcher
        ↓
Kotlin coroutine adapter
        ↓
SharedFlow / StateFlow
        ↓
ViewModel
        ↓
Compose
```

---

# 10. Event Threading Rule

Never update Compose state from arbitrary Rust worker threads directly.

JNI adapter hands events into a controlled coroutine context.

---

# 11. Event Coalescing

High-frequency events such as:

```text
transfer progress
call quality
audio levels
typing
presence
```

must be throttled/coalesced before reaching Compose.

---

# 12. Android App Shell

Recommended phone shell:

```text
Scaffold
├── TopAppBar
├── NavHost
├── BottomNavigation
└── SnackbarHost
```

Primary destinations:

```text
Chats
Calls
Contacts
```

Secondary destinations:

```text
Files
Search
Devices
Settings
```

---

# 13. Bottom Navigation

Use only a small number of primary destinations.

Do not place:

```text
8–10 tabs
```

in bottom navigation.

---

# 14. Primary Destinations

Recommended:

```text
Chats
Calls
Contacts
```

Optional fourth:

```text
Files
```

if product usage justifies it.

---

# 15. Secondary Destinations

Access through:

```text
top app bar
overflow
drawer/rail on larger devices
search entry
settings hierarchy
```

---

# 16. Tablet / Foldable Navigation

Use adaptive structure:

```text
NavigationRail
+
List Pane
+
Detail Pane
```

where width allows.

---

# 17. Window Size Classes

Android shell should react to:

```text
Compact
Medium
Expanded
```

using platform window-size/adaptive APIs.

---

# 18. Compact Phone

Typical layout:

```text
single main pane
bottom navigation
full-screen destination changes
```

---

# 19. Medium Width

Potential:

```text
rail
+
single content pane
```

or:

```text
list/detail
```

depending feature.

---

# 20. Expanded Width

Use:

```text
navigation rail
list pane
detail pane
```

for conversations, contacts, files, settings.

---

# 21. Foldables

Respect:

```text
hinge
fold posture
separating fold
```

Avoid placing critical controls under hinge.

---

# 22. Orientation

Support portrait/landscape gracefully.

Do not lock orientation globally except feature-specific reasons such as some camera modes.

---

# 23. Typed Navigation

Define stable destinations.

```kotlin
sealed interface AppDestination
```

Conceptually:

```text
Chats
Conversation(id)
Calls
Call(id)
Contacts
Contact(id)
Files
File(id)
Search
Devices
Device(id)
Security
Settings(page)
```

---

# 24. No Raw Route Strings in Feature Logic

Route strings are navigation implementation detail.

Feature code should use typed destinations.

---

# 25. Navigation Ownership

Compose owns back stack.

Rust may emit semantic navigation requests for:

```text
incoming call
security event
notification click
deep link
```

but should not own Android back stack.

---

# 26. Predictive Back

Integrate with Android predictive back.

Do not implement a custom incompatible gesture stack.

---

# 27. Back Behavior

Examples:

```text
conversation → chats
contact detail → contacts
search result → previous screen
modal sheet → dismiss
```

---

# 28. Back During Active Call

Back should:

```text
leave full call UI
```

without hanging up.

Call remains active in foreground service/PiP if policy allows.

---

# 29. Deep Links

Potential:

```text
conversation
call
device
file
security event
backup import
plugin package
```

All external deep links are untrusted input and validated before navigation.

---

# 30. Notification Click

Part 31:

```text
notification intent
→ Kotlin parser
→ typed destination
→ Rust state validation
→ navigation
```

---

# 31. Cold Start Deep Link

If app process not running:

```text
Activity created
    ↓
Rust runtime starts
    ↓
pending destination stored
    ↓
core reaches Ready
    ↓
navigate
```

---

# 32. Pending Launch Action

Use:

```kotlin
sealed interface PendingLaunchAction
```

Examples:

```text
OpenConversation
OpenCall
OpenSecurityEvent
ImportBackup
```

---

# 33. MainActivity

Should remain thin.

Responsibilities:

```text
Compose root
platform launch intents
window setup
lifecycle handoff
```

Not business logic.

---

# 34. Application Class

Handles:

```text
global Android initialization
Rust library loading
notification channels
dependency graph
```

Avoid opening full runtime unnecessarily before needed.

---

# 35. Rust Library Load

Load native library once:

```kotlin
System.loadLibrary(...)
```

JNI ABI handshake validates compatibility.

---

# 36. Native ABI Handshake

At startup compare:

```text
Android wrapper expected ABI
Rust native actual ABI
```

Mismatch:

```text
fatal startup error
```

with clear recovery/update guidance.

---

# 37. Rust Core States

Android observes:

```rust
pub enum CoreState {
    Starting,
    Ready,
    Degraded,
    Unavailable,
}
```

---

# 38. Starting UI

Render app shell quickly.

Show:

```text
Starting…
```

only where content unavailable.

---

# 39. Ready

Normal operation.

---

# 40. Degraded

Examples:

```text
search rebuilding
relay unavailable
plugin failed
backup stale
```

Do not block core messaging.

---

# 41. Unavailable

Serious error.

Show recovery screen:

```text
Retry
Diagnostics
Restore Backup
```

---

# 42. Process Death

Android can kill process at any time.

The UI architecture must assume:

```text
Activity disappears
ViewModel disappears
JNI process state disappears
```

unless a separate service/process remains.

---

# 43. Durable Truth

On process restart:

```text
Rust reloads durable state
```

Compose reconstructs from new snapshots.

---

# 44. SavedStateHandle

Use only for presentation/navigation state such as:

```text
selected conversation ID
draft ID
scroll anchor
active settings page
temporary filter state
```

---

# 45. Do Not Save Stale Ephemeral State

Do not restore:

```text
typing
presence
live decoder handles
call transport connection
Surface
temporary permissions result
```

as if still valid.

---

# 46. Call Survival

If active call must survive Activity recreation/background:

```text
foreground service
+
Rust call runtime
```

can outlive UI surface.

---

# 47. Activity Recreation

Examples:

```text
rotation
theme change
window resize
```

should not restart:

```text
call
transfer
sync
```

---

# 48. Compose Recomposition

Must not trigger side effects automatically.

Do not call:

```text
sendMessage()
startCall()
registerPush()
```

from ordinary recomposition.

Use:

```text
event handlers
LaunchedEffect with stable keys
ViewModel init
```

carefully.

---

# 49. Screen State Model

Each screen gets:

```kotlin
data class ScreenUiState(...)
```

with:

```text
content
loading
error
local presentation state
```

---

# 50. StateFlow

ViewModel exposes:

```kotlin
StateFlow<ScreenUiState>
```

Compose uses:

```text
collectAsStateWithLifecycle
```

or equivalent lifecycle-aware collection.

---

# 51. One-Off Effects

Use separate effect stream for:

```text
show snackbar
open picker
request permission
navigate
```

Do not encode one-time events permanently inside `UiState`.

---

# 52. Effect Stream

```text
SharedFlow<UiEffect>
```

or channel-based equivalent.

---

# 53. Effect Examples

```text
RequestCameraPermission
OpenFilePicker
ShowSnackbar
LaunchShareSheet
OpenSystemSettings
EnterPiP
```

---

# 54. Platform Request Pattern

Rust may return semantic need:

```text
CameraPermissionRequired
```

ViewModel emits:

```text
RequestCameraPermission
```

Compose/Activity executes platform request.

---

# 55. Permission Architecture

Centralize permissions.

Examples:

```text
Camera
Microphone
Notifications
Bluetooth Scan
Bluetooth Connect
Nearby Wi-Fi
NFC
Storage/Media where required
```

---

# 56. Permission State

```kotlin
enum class PermissionState {
    Granted,
    Denied,
    NeedsRequest,
    PermanentlyDenied
}
```

---

# 57. Permission UX

Ask contextually.

Examples:

```text
camera → when starting video
microphone → when starting call/voice note
Bluetooth → when using nearby
notifications → when enabling background alerts
```

---

# 58. Avoid Permission Dump

Do not request every possible permission on first launch.

---

# 59. Permanently Denied

Show:

```text
Open Settings
```

with explanation.

---

# 60. Permission Denial Degradation

Examples:

```text
camera denied → audio call still possible
microphone denied → receive-only call or cannot call
notifications denied → app works but background alerts may fail
```

---

# 61. IME / Keyboard

Compose composer must handle:

```text
IME actions
multiline text
hardware keyboard
emoji
RTL
selection
```

---

# 62. Composer Bottom Insets

Use proper:

```text
IME insets
navigation bars
gesture areas
```

so composer stays visible.

---

# 63. Keyboard Send Policy

Examples:

```text
phone Enter → newline
send button → send
hardware Ctrl+Enter → send
```

Configurable later.

---

# 64. Message Timeline

Use:

```text
LazyColumn
stable keys
paging
scroll anchoring
```

---

# 65. Stable Key

Always:

```text
MessageId
```

---

# 66. New Messages

At bottom:

```text
follow
```

Reading history:

```text
show new message chip
```

---

# 67. Read Detection

Part 30 semantics.

Use:

```text
resumed lifecycle
visible conversation
visible message range
```

---

# 68. Conversation List

Use:

```text
LazyColumn
```

with:

```text
avatar
title
preview
timestamp
unread
mute
presence optional
```

---

# 69. Pull to Refresh

Do not imply network is source of truth.

If present:

```text
trigger sync/refresh
```

but local data remains visible.

---

# 70. Swipe Actions

Optional Android-native interaction for:

```text
archive
mute
mark read
```

Provide alternate accessible action.

---

# 71. Long Press

Use for message/conversation context actions.

---

# 72. Bottom Sheets

Good for:

```text
attachment picker
message actions
audio route
conversation options
```

---

# 73. Dialogs

Use for:

```text
destructive/security decisions
```

not every action.

---

# 74. Snackbars

Use for:

```text
undo
short confirmation
retryable minor failure
```

---

# 75. System File Picker

Use Android document/media picker where possible.

Do not create broad storage access if not needed.

---

# 76. File Picker Flow

```text
Compose action
→ ActivityResult launcher
→ URI
→ safe FD/content handle
→ Rust file subsystem
```

---

# 77. No Giant ByteArray

Never convert large selected files into:

```text
ByteArray
```

for JNI.

Pass:

```text
file descriptor
content URI handle
stream bridge
```

---

# 78. Share Sheet — Outgoing

Rust/Compose can request:

```text
share message text
share file
share invite
```

Kotlin launches system share sheet.

---

# 79. Share Intent — Incoming

Android share intent:

```text
ACTION_SEND / ACTION_SEND_MULTIPLE
```

maps to typed share request.

---

# 80. Incoming Share Flow

```text
external app
→ Android intent
→ Kotlin parse
→ safe handles
→ Compose choose conversation
→ Rust send command
```

---

# 81. External Intent Validation

Treat all incoming intent data as untrusted.

---

# 82. Notifications

Part 31 Kotlin layer owns platform notification APIs.

Rust owns:

```text
whether notification should exist
privacy level
dedup
semantic actions
```

---

# 83. Notification Channels

Kotlin creates stable channels:

```text
Messages
Calls
Security
Emergency
Transfers
```

---

# 84. Incoming Call Notification

Flow:

```text
push wake
→ Rust fetch/authenticate offer
→ Kotlin show call notification
```

Do not ring based solely on push metadata.

---

# 85. Accept Call from Notification

```text
PendingIntent
→ Android service/activity
→ Rust CallController.accept(callId)
→ foreground service
→ call UI
```

---

# 86. Decline Call

Can often complete headlessly.

---

# 87. Foreground Service

Use for:

```text
active audio/video call
long user-visible transfer where justified
```

Not to keep idle messenger permanently alive.

---

# 88. CallForegroundService

Responsibilities:

```text
service lifecycle
foreground notification
audio focus integration
Rust call-runtime binding
```

It does not own call state.

---

# 89. Picture-in-Picture

Active video call can enter PiP.

Rust call remains active.

Compose Activity may:

```text
enterPiPMode
```

---

# 90. PiP State

UI adapts:

```text
minimal controls
video focus
```

---

# 91. Return from PiP

Rebuild full call UI from Rust `CallSnapshot`.

---

# 92. Screen Rotation During Call

Surface may recreate.

Part 25 handles renderer rebinding.

Call stays active.

---

# 93. Camera Surface

Compose owns view placement.

Rust owns media session.

No raw frames in Compose state.

---

# 94. Audio Route

Compose may expose chooser:

```text
speaker
earpiece
Bluetooth
wired
```

Rust/Part 26 remains authoritative.

---

# 95. Bluetooth Permissions

Handle Android-version differences in Kotlin platform layer.

Rust receives normalized capability/result.

---

# 96. NFC

Android NFC intent/reader mode lives in Kotlin adapter.

Rust receives:

```text
bootstrap payload
```

for Part 15 validation.

---

# 97. Nearby Discovery

Compose renders nearby devices.

Rust/proximity layer owns:

```text
discovery state
identity verification
connection policy
```

---

# 98. QR Scanner

Two options:

```text
Compose camera integration
platform camera/scanner component
```

Parsed payload goes to Rust bootstrap service.

---

# 99. Camera Permission

Requested only when scanner/video feature begins.

---

# 100. Biometric Authentication

Use Android `BiometricPrompt` or appropriate platform API.

Rust requests semantic authentication:

```text
AuthenticateForRecoveryKey
AuthenticateForDeviceRevocation
```

Kotlin runs prompt and returns result.

---

# 101. Sensitive Screens

Examples:

```text
recovery key
device revoke
identity reset
backup key
```

may require re-authentication.

---

# 102. Screenshot Security

For highly sensitive screens, Kotlin may apply secure-window policy if product chooses.

---

# 103. Clipboard

Use Android clipboard for explicit copy actions.

Avoid auto-copying sensitive secrets.

---

# 104. App Lock

Optional future:

```text
biometric/device credential
```

before opening sensitive UI.

Rust core may remain running while UI locked.

---

# 105. Theme

Support:

```text
System
Light
Dark
```

Compose implementation may build on Material 3.

---

# 106. Dynamic Color

Optional Android feature.

Product may support:

```text
system dynamic color
```

or fixed brand theme.

---

# 107. Design Tokens

Map shared semantic design language into Compose:

```text
color scheme
typography
shape
spacing
motion
```

---

# 108. Font Scaling

Must support system font scale.

Avoid fixed-height components that clip at large text.

---

# 109. Touch Targets

Respect minimum touch target size.

---

# 110. Accessibility Semantics

Use Compose semantics for:

```text
buttons
message rows
status
selection
call controls
security warnings
```

---

# 111. TalkBack Message Row

Announce coherent unit:

```text
Alice, 10:42 AM, Hello, delivered
```

---

# 112. Live Regions

Use carefully for:

```text
new messages
call connection changes
security alert
```

Do not overwhelm screen reader.

---

# 113. Reduced Motion

Respect Android animation scale/reduced motion where available.

---

# 114. High Contrast

Do not rely solely on subtle color differences.

---

# 115. RTL

Support:

```text
Arabic
Urdu
Hebrew
```

layout direction.

Message bubbles and icons should adapt correctly.

---

# 116. Localization

All strings in Android resources.

Rust returns:

```text
codes
structured context
timestamps
numbers
```

Compose localizes.

---

# 117. Date/Time Formatting

Android UI uses locale/device settings.

---

# 118. Error Architecture

ViewModel receives structured `UiError`.

Maps to:

```text
snackbar
inline error
dialog
full recovery screen
```

---

# 119. Transient Error

Example:

```text
peer unreachable
```

show small non-blocking status.

---

# 120. Permission Error

Show contextual explanation/action.

---

# 121. Security Error

Use strong hierarchy and block risky continuation.

---

# 122. Storage Error

Example:

```text
Storage full
```

show:

```text
Manage Storage
```

---

# 123. Offline

Offline is normal.

Conversation list/history remains available.

Queued messages render normally.

---

# 124. Connection Status

Use subtle:

```text
Offline
Reconnecting
```

only when useful.

---

# 125. Empty States

Examples:

```text
No conversations
No calls
No contacts
No files
No search results
```

with one relevant action.

---

# 126. Loading

Prefer local data immediately.

Use skeleton/progress only for data actually unavailable.

---

# 127. Pulling Rust Snapshot

Screen open:

```text
ViewModel starts
→ repository requests snapshot
→ state renders
→ event subscription begins
```

---

# 128. Lifecycle-Aware Subscription

Collect event streams only while screen lifecycle requires them.

---

# 129. Global Events

Keep app-wide:

```text
active call
security alert
core health
```

---

# 130. Screen-Scoped Events

Examples:

```text
typing for current conversation
file transfer detail
contact presence
```

---

# 131. Avoid Subscription Leaks

ViewModel clears/subscription cancels when feature no longer active.

---

# 132. Navigation + ViewModel Scope

Conversation ViewModel scoped to conversation destination.

Chats list ViewModel scoped to chats graph.

---

# 133. Shared ViewModel

Use only when state truly spans destinations.

Avoid global mega-ViewModel.

---

# 134. Recommended ViewModel Pattern

Per feature:

```text
UiState
UiAction
UiEffect
```

---

# 135. UI Action

Example:

```kotlin
sealed interface ConversationAction {
    data class Send(val text: String) : ConversationAction
    data class Retry(val messageId: String) : ConversationAction
    data object StartAudioCall : ConversationAction
}
```

---

# 136. ViewModel Handling

ViewModel translates action into:

```text
Rust semantic command
```

---

# 137. UI Effect

Examples:

```text
OpenPicker
RequestPermission
Navigate
ShowSnackbar
```

---

# 138. No Domain Decision in Composable

Composable renders and emits actions.

---

# 139. Previewability

Compose screens should support previews using fake UI state.

No live Rust runtime required.

---

# 140. Testkit

Create Kotlin fake repository matching Rust presentation contract.

---

# 141. Compose Preview Models

Provide:

```text
empty
loaded
offline
error
large font
RTL
```

samples.

---

# 142. Android Testing

Required:

```text
ViewModel unit tests
Compose UI tests
navigation tests
permission tests
deep-link tests
process-death tests
PiP tests
foreground service tests
JNI adapter tests
```

---

# 143. Process-Death Test

Scenario:

```text
open conversation
type draft
background
kill process
restore
```

Expected:

```text
navigation/draft recovery according to policy
core state reloaded
```

---

# 144. Call Recreation Test

During call:

```text
rotate
background
return
```

Call remains.

---

# 145. Notification Cold Start Test

Tap message notification from killed app.

Correct conversation opens after Rust ready.

---

# 146. Deep-Link Security Test

Malformed external deep link does not crash or access unauthorized data.

---

# 147. Permission Denial Test

Camera denied.

Video call degrades gracefully to audio if allowed.

---

# 148. Notification Permission Denied

App works.

Diagnostics explain possible background-call/message limitations.

---

# 149. Share Intent Test

Large shared file uses handle/FD, not memory copy.

---

# 150. Foldable Test

Hinge does not obscure list/detail content.

---

# 151. Large Font Test

At large system font:

```text
no clipped buttons
no hidden security text
```

---

# 152. TalkBack Test

Core flows:

```text
open chat
read message
send message
accept call
verify device
restore backup
```

---

# 153. Performance

Measure:

```text
cold app shell startup
conversation open
LazyColumn scroll
search
call UI updates
```

---

# 154. Compose Recomposition Budget

Avoid broad state objects that recompose whole app for:

```text
typing tick
transfer percentage
```

Split state into stable feature-level flows.

---

# 155. Stable Data Models

Use immutable Kotlin UI models.

---

# 156. Mapping Layer

Rust DTO:

```text
ConversationSummaryDto
```

maps once to Kotlin `ConversationUiModel`.

Avoid repeated expensive conversions in Composable.

---

# 157. Pagination

Rust provides page/cursor.

Compose list requests older/newer pages via ViewModel.

---

# 158. Load More

Use scrolling thresholds.

---

# 159. Search Debounce

ViewModel debounces user query.

Rust search remains authoritative.

---

# 160. File Thumbnail Loading

Use thumbnail handle/URI generated by trusted file subsystem.

Avoid reading full original media in Composable.

---

# 161. Image Cache

Android image loader can cache UI thumbnails.

Authoritative file state remains Rust.

---

# 162. Video Playback

For shared video attachment, Compose may host native player/view.

Do not route frames through Kotlin state.

---

# 163. System Back During Modal

Dismiss:

```text
bottom sheet
dialog
selection mode
```

before leaving destination.

---

# 164. Selection Mode

Android contextual selection for:

```text
messages
files
contacts
```

---

# 165. Haptics

Use sparingly for:

```text
long press
successful pairing
destructive confirmation
```

---

# 166. Vibration Policy

Respect system/user settings.

---

# 167. Notification vs In-App Banner

Foreground app:

```text
in-app banner/snackbar
```

Background:

```text
system notification
```

Rust notification policy decides semantic intent.

---

# 168. Screen-Share Privacy

If active screen share:

```text
notification previews may switch to Generic
```

according to Part 31 policy.

---

# 169. App-Specific Status Bar

Keep status bar/system bars integrated with theme and edge-to-edge layout.

---

# 170. Edge-to-Edge

Use Android edge-to-edge correctly with insets.

---

# 171. Gesture Navigation

Respect system gesture areas.

Do not place tiny critical controls against gesture edges.

---

# 172. Safe Insets

Apply to:

```text
composer
call controls
bottom bar
sheets
```

---

# 173. Android Settings Shell

On phone:

```text
settings list
→ detail screen
```

On tablet:

```text
settings list pane
+
detail pane
```

---

# 174. Android Security Center

Can adapt similarly:

```text
devices list
security events
recovery
```

---

# 175. Backup UI

Background operations report through Rust.

Compose shows:

```text
progress
destination
verification
errors
```

---

# 176. Restore UI

High-risk operation.

Require:

```text
preview
compatibility check
confirmation
progress
```

---

# 177. Emergency UX

Part 17 data.

Android shell should make SOS accessible without accidental triggering.

---

# 178. Lock-Screen Emergency

If product later supports:

```text
notification action
shortcut
widget
```

must route into verified Rust emergency command.

---

# 179. App Widget

Optional future.

Widget may show:

```text
unread count
quick contact
SOS
```

but no sensitive content by default.

---

# 180. Shortcuts

Android app shortcuts can include:

```text
New Message
Recent Contact
Scan QR
```

---

# 181. Shortcut Security

External shortcut intent validated.

---

# 182. Compose Navigation Restoration

Navigation library should restore back stack where safe.

Do not restore dead `CallId` as active call without Rust validation.

---

# 183. Stale Destination

If restoring:

```text
ConversationId deleted
```

navigate to safe fallback.

---

# 184. Stale Call

If call ended while Activity gone:

```text
show call history/ended state
```

not active UI.

---

# 185. Stale Device

If device revoked/deleted:

```text
open security center
```

with explanatory message.

---

# 186. Background Wake

Part 31 may start headless Rust work while no Activity exists.

When UI launches later:

```text
durable snapshots already include received messages
```

No special Compose synchronization needed.

---

# 187. Service-to-UI Handoff

Foreground call service can expose active call through Rust global state.

Activity/ViewModel reads fresh snapshot.

---

# 188. Single Activity

Recommended:

```text
single-activity Compose app
```

with feature screens in navigation graph.

Use extra Activity only for exceptional platform requirements.

---

# 189. Separate Call Activity?

Usually unnecessary if foreground service + Compose navigation suffice.

Consider only if Android call UX/system integration benefits.

---

# 190. Activity Result APIs

Use for:

```text
permissions
file picker
document creation
share result
```

rather than legacy callbacks.

---

# 191. Dependency Boundaries

Feature UI depends on:

```text
UI models
repository interfaces
platform abstractions
```

not directly on JNI internals.

---

# 192. Rust Repository Interface

```kotlin
interface RustRepository {
    fun conversations(): Flow<List<ConversationUiModel>>
    suspend fun sendMessage(command: SendMessageUiCommand)
    suspend fun startCall(peer: PeerUiId)
}
```

Implementation wraps JNI.

---

# 193. Test Repository

Fake implementation for:

```text
Compose previews
unit tests
offline fixtures
```

---

# 194. No JNI in Composables

Hard rule.

Composable should never call native methods directly.

---

# 195. No Android Context in Rust Core

Hard rule.

Android context remains in Kotlin platform layer.

---

# 196. JNI Error Translation

Rust error code:

```text
PermissionRequired
PeerUnavailable
SecurityMismatch
```

maps to Kotlin sealed error.

---

# 197. Kotlin Error Model

```kotlin
sealed interface UiError
```

with fields:

```text
severity
retryable
action
```

---

# 198. State Restoration Priority

Restore in order:

```text
core truth
navigation
drafts
scroll
temporary filters
```

---

# 199. Draft Architecture

Draft may live:

```text
ViewModel transient
```

or later:

```text
Rust durable draft service
```

For process-death resilience, durable draft service is preferable for important conversations.

---

# 200. Scroll Restoration

Save:

```text
anchor MessageId
offset
```

not absolute list index.

---

# 201. Conversation List Scroll

Save lightweight list position locally.

---

# 202. Search State

Can restore:

```text
query
filters
selected result
```

if desired.

---

# 203. Sensitive Search

High-security profile may not persist search query.

---

# 204. Metrics

UI performance metrics may include:

```text
screen render latency
JNI call duration
event queue lag
recomposition count
```

No message content.

---

# 205. Diagnostics

Developer screen can show:

```text
JNI connected
core state
event backlog
foreground service state
notification permission
push registration
```

---

# 206. Crash Reports

Redact:

```text
message body
contact names
file names
recovery material
```

where possible.

---

# 207. Release Quality Gate

Android UI release should verify:

```text
navigation
process death
permissions
notifications
calls
PiP
share intents
foldables
TalkBack
large font
RTL
dark/light
```

---

# 208. Suggested Android Feature Module Layout

```text
feature/
├── chats/
│   ├── ChatsRoute.kt
│   ├── ChatsScreen.kt
│   ├── ChatsViewModel.kt
│   └── ChatsUiState.kt
├── conversation/
├── calls/
├── contacts/
├── files/
├── search/
├── devices/
├── security/
└── settings/
```

---

# 209. Route vs Screen

`Route` handles:

```text
ViewModel
navigation
effects
```

`Screen` handles:

```text
pure UI
```

This keeps Composables testable.

---

# 210. Example Pattern

```text
ConversationRoute
    ↓
collect ViewModel state
    ↓
ConversationScreen(
    state,
    onAction
)
```

---

# 211. Pure Screen

`ConversationScreen` does not know:

```text
JNI
Rust
repository
navigation controller
```

---

# 212. Previewability

This allows rich Compose previews for:

```text
normal
offline
error
large font
RTL
group
```

---

# 213. Cross-Platform Semantic Parity

Desktop Dioxus and Android Compose should expose equivalent:

```text
message states
call states
security warnings
transfer states
receipt semantics
```

even if interaction differs.

---

# 214. Android-Specific UX Freedom

Compose may use:

```text
swipe
bottom sheet
system back
PiP
system picker
```

without forcing those concepts into desktop.

---

# 215. Definition of Done

UI/UX Part 03 is complete when:

- Android UI is pure Kotlin + Jetpack Compose
- Rust remains the authoritative product/runtime core
- JNI exposes coarse semantic APIs instead of low-level internals
- ViewModels convert Rust snapshots/events into lifecycle-aware `StateFlow`
- Composables never call JNI directly
- a typed Compose navigation model exists
- predictive back integrates with Android system behavior
- phone, tablet, and foldable adaptive shells are defined
- Activity/process death is treated as normal
- `SavedStateHandle` stores only presentation/navigation state
- stale ephemeral state is never restored as truth
- permissions are contextual and platform-native
- file/share flows pass handles/FDs rather than giant byte arrays
- incoming calls are authenticated before UI rings
- active calls can survive Activity recreation and use foreground-service/PiP integration
- raw media never enters ordinary Compose state
- notification, deep-link, and external intent inputs are validated
- accessibility, TalkBack, font scaling, RTL, reduced motion, touch targets, and edge-to-edge insets are first-class
- screen UI follows `UiState + UiAction + UiEffect`
- process-death, deep-link, call recreation, permission denial, foldable, TalkBack, and large-font tests are release requirements

---

# 216. Final Android Architecture

```text
                     RUST CORE
                         │
               Presentation Service
                         │
                      JNI
                         │
                 RustRepository
                         │
                     ViewModel
              ┌──────────┴──────────┐
              │                     │
           StateFlow              UiEffect
              │                     │
              ▼                     ▼
       Jetpack Compose       Android Platform
              │               Permissions
              │               Picker
              │               Notifications
              │               PiP
              │               Biometrics
              ▼
        Android UI Shell
```

Phone:

```text
Bottom Navigation
        ↓
    NavHost
        ↓
Single-pane screen
```

Tablet/foldable:

```text
Navigation Rail
        ↓
List Pane
        ↓
Detail Pane
```

---

# 217. Final Principle

The Android application should feel like a native Android product while remaining a thin presentation client over the Rust system.

The correct split is:

```text
Rust:
    state
    business rules
    networking
    storage
    security
    calls
    sync
    search
    backup

Kotlin:
    lifecycle
    permissions
    Android services
    intents
    system UI integration

Jetpack Compose:
    presentation
    interaction
    navigation
    adaptive layouts
```

This gives Android the platform quality users expect without duplicating the communication engine outside Rust.
