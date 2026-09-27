# UI/UX Part 02 — Desktop Dioxus App Shell, Navigation & Window Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 02  
**Platform:** Desktop  
**UI framework:** Dioxus  
**Primary language:** Rust  
**Primary goal:** define the complete desktop application shell, navigation model, multi-pane layout, multi-window behavior, keyboard-first interaction, window persistence, command palette, system tray, drag-and-drop, context menus, local daemon integration, accessibility, and responsive desktop behavior.

---

# 1. Purpose

The desktop application is not merely a larger version of the mobile UI.

Desktop users expect:

```text
persistent navigation
split panes
keyboard shortcuts
mouse/trackpad
drag-and-drop
multi-window
system tray
context menus
large-screen density
resizable panels
command palette
```

The desktop shell must therefore be designed as a dedicated interaction system while still consuming the same Rust core defined in UI/UX Part 01.

The governing principle is:

> **The desktop shell should feel like a fast native communication workspace, not a phone layout stretched across a large monitor.**

---

# 2. Architectural Position

```text
Rust Core
   │
Presentation Service
   │
Desktop Presenter
   │
Dioxus Shell
   │
┌──┴─────────────────────────────┐
│ Navigation                     │
│ Workspace Panes                │
│ Windows                        │
│ Command Palette                │
│ Tray                           │
│ Menus                          │
│ Keyboard Shortcuts             │
│ Accessibility                  │
└────────────────────────────────┘
```

---

# 3. Desktop App Shell Responsibilities

The shell owns:

```text
window topology
workspace layout
navigation
pane sizing
focus
keyboard routing
menu/context menus
command palette
tray
deep-link routing
presentation-state persistence
```

It does not own:

```text
messages
sync truth
call truth
transfer truth
security truth
search truth
```

Those remain in Rust core/presentation services.

---

# 4. Recommended High-Level Layout

```text
+------------------------------------------------------------------+
| App Chrome / Window Bar / Global Actions                         |
+------+----------------------+------------------------------------+
| Rail | Secondary Sidebar    | Main Workspace                     |
|      |                      |                                    |
|      | Chats / Calls /      | Conversation / Call / File /      |
|      | Contacts / Files     | Search / Security / Settings      |
|      |                      |                                    |
+------+----------------------+------------------------------------+
| Status / Optional Diagnostics / Transfer Summary                 |
+------------------------------------------------------------------+
```

Optional fourth pane:

```text
Details / Inspector
```

---

# 5. Primary Navigation Rail

Recommended top-level destinations:

```text
Chats
Calls
Contacts
Files
Search
```

Secondary destinations lower in rail:

```text
Devices
Plugins
Settings
```

Optional:

```text
Emergency
Diagnostics
```

based on product role.

---

# 6. Rail Behavior

Rail can support:

```text
icon-only compact mode
icon + label expanded mode
```

User preference can persist.

---

# 7. Rail Selection

Selecting primary destination changes:

```text
secondary sidebar content
main workspace default
```

---

# 8. Secondary Sidebar

Examples:

## Chats

```text
Pinned
Recent
Archived
Requests
```

## Calls

```text
Recent Calls
Missed
Active
```

## Contacts

```text
Contacts
Requests
Nearby
Blocked
```

## Files

```text
Recent
Images
Videos
Documents
Audio
Links
Transfers
```

---

# 9. Main Workspace

Main workspace renders the active entity:

```text
conversation
contact
call
file
search result
security event
device
plugin
settings page
```

---

# 10. Optional Details Pane

Useful for:

```text
conversation info
shared media
contact verification
group members
file details
call diagnostics
security details
```

---

# 11. Inspector Visibility

User toggles with:

```text
toolbar button
keyboard shortcut
```

The inspector should not permanently consume space in medium-width windows.

---

# 12. Responsive Desktop Layout

Use window-width classes:

```rust
pub enum DesktopWindowClass {
    Compact,
    Medium,
    Wide,
    UltraWide,
}
```

---

# 13. Compact Desktop

Examples:

```text
small laptop window
narrow snapped window
```

Layout:

```text
rail
single primary pane
```

Secondary sidebar may overlay or replace workspace.

---

# 14. Medium Desktop

Layout:

```text
rail
secondary list
main workspace
```

No permanent inspector.

---

# 15. Wide Desktop

Layout:

```text
rail
secondary list
main workspace
optional inspector
```

---

# 16. UltraWide Desktop

Can support:

```text
rail
secondary sidebar
main workspace
details pane
optional utility panel
```

Do not automatically fill every pixel with content.

---

# 17. Minimum Window Size

Define practical minimum.

Below minimum:

```text
show constrained compact layout
```

not broken overlapping panes.

---

# 18. Window Size Persistence

Persist:

```text
width
height
position
maximized state
last monitor
```

locally.

---

# 19. Multi-Monitor

On restart:

```text
if old monitor absent
→ reposition safely on available display
```

Never restore window off-screen.

---

# 20. Window Types

Recommended:

```rust
pub enum DesktopWindowKind {
    Main,
    Call,
    MediaViewer,
    Settings,
    Diagnostics,
    ComposeDetached,
}
```

---

# 21. Main Window

Canonical workspace.

Usually one instance.

---

# 22. Call Window

Optional separate floating window for active call.

Can be:

```text
always-on-top optional
compact mode
video-focused
```

---

# 23. Media Viewer Window

Useful for:

```text
images
videos
documents
```

Can detach from main app.

---

# 24. Settings Window

Two valid patterns:

```text
settings in main workspace
```

or:

```text
separate settings window
```

Recommendation:

```text
main workspace first
```

Separate window only if desktop UX benefits.

---

# 25. Diagnostics Window

Advanced/developer use.

Can be detachable.

---

# 26. Window Ownership

Window state is presentation state.

Rust core remains independent.

---

# 27. Active Call Across Windows

If user closes call window:

```text
call does not necessarily end
```

Product policy can:

```text
minimize call
return to main workspace
or hang up only on explicit action
```

---

# 28. Window Close Semantics

Main window close can mean:

```text
exit app
or
hide to tray
```

based on user setting.

---

# 29. System Tray

Recommended desktop feature.

Tray menu:

```text
Open
New Message
Active Call
Pause/Resume Notifications
Quit
```

Optional status:

```text
Online
Offline
Syncing
```

---

# 30. Tray and Daemon Mode

If daemon remains running after UI closes:

```text
tray controls daemon/UI
```

---

# 31. Exit Semantics

Distinguish:

```text
Close Window
Hide to Tray
Quit UI
Stop Daemon
Quit Entire App
```

Do not make these ambiguous.

---

# 32. Keyboard-First UX

Desktop should be fully operable without mouse.

Core shortcuts:

```text
Ctrl/Cmd+K → Command Palette
Ctrl/Cmd+N → New Conversation
Ctrl/Cmd+F → Search Current Context
Ctrl/Cmd+Shift+F → Global Search
Ctrl/Cmd+, → Settings
Ctrl/Cmd+W → Close tab/pane
Ctrl/Cmd+Shift+W → Close window
Esc → Dismiss / Back
```

---

# 33. Conversation Shortcuts

Examples:

```text
Alt+Up/Down → Previous/Next conversation
Ctrl/Cmd+Enter → Send
Ctrl/Cmd+Shift+R → Reply
Ctrl/Cmd+E → Edit own last message
```

Exact bindings configurable.

---

# 34. Call Shortcuts

Examples:

```text
Ctrl/Cmd+Shift+M → Mute
Ctrl/Cmd+Shift+V → Video toggle
Ctrl/Cmd+Shift+H → Hang up
```

---

# 35. Shortcut Conflict Handling

Check OS conventions.

Allow customization later.

---

# 36. Shortcut Registry

```rust
pub struct ShortcutRegistry {
    // command → binding
}
```

---

# 37. Command Palette

One of the strongest desktop productivity features.

Open:

```text
Ctrl/Cmd+K
```

---

# 38. Command Palette Search

Commands can include:

```text
New conversation
Open contact
Open settings
Start call
Search messages
Open transfers
Toggle inspector
Open diagnostics
```

---

# 39. Command Palette Data Sources

```text
static app commands
recent conversations
contacts
files
settings destinations
```

---

# 40. Palette Result Types

```rust
pub enum CommandPaletteItem {
    Command(...),
    Conversation(...),
    Contact(...),
    File(...),
    Setting(...),
}
```

---

# 41. Keyboard Navigation

Palette:

```text
Up/Down
Enter
Esc
```

---

# 42. Fuzzy Search

Use lightweight fuzzy matching.

No semantic search required for command palette.

---

# 43. Tabs

Desktop may benefit from tabs for:

```text
multiple conversations
searches
files
settings
```

---

# 44. Tab Model

```rust
pub struct WorkspaceTab {
    pub id: TabId,
    pub destination: DesktopDestination,
    pub pinned: bool,
}
```

---

# 45. Tab Scope

Recommendation:

```text
optional advanced desktop feature
```

Do not force tabs if split-pane workspace already works well.

---

# 46. Pinned Tabs

Useful for:

```text
frequent conversation
monitoring transfer
diagnostics
```

---

# 47. Tab Persistence

Persist only stable destinations.

Do not restore stale:

```text
typing
call media handle
temporary modal
```

---

# 48. Navigation History

Maintain per-tab:

```text
back
forward
```

for flows like:

```text
Search
→ Message
→ Contact
→ Back
```

---

# 49. Navigation Destination

```rust
pub enum DesktopDestination {
    Chats,
    Conversation(ConversationId),
    Calls,
    Call(CallId),
    Contacts,
    Contact(AccountId),
    Files,
    File(BlobId),
    Search(SearchStateId),
    Devices,
    Device(DeviceId),
    Security,
    Settings(SettingsPage),
    Plugin(PluginId),
    Diagnostics,
}
```

---

# 50. Typed Navigation

Do not navigate by arbitrary strings.

---

# 51. Deep Links

External/opened links should resolve into typed destinations.

Examples:

```text
conversation
call
device
file
security event
```

---

# 52. Deep-Link Validation

Untrusted external link input must be validated before navigation.

---

# 53. Sidebar List Virtualization

Conversation/contact/file lists can grow large.

Virtualize.

---

# 54. Row Density

Desktop supports:

```text
Comfortable
Compact
```

later.

Default should be readable, not overly sparse.

---

# 55. Conversation Row

Recommended information:

```text
avatar
title
last-message preview
timestamp
unread count
mute indicator
presence optionally
```

---

# 56. Row Selection

Single click:

```text
open
```

Double click:

```text
optional detach/new tab
```

depending UX.

---

# 57. Multi-Selection

Useful in:

```text
files
contacts
messages
```

Support:

```text
Ctrl/Cmd click
Shift click
```

---

# 58. Context Menus

Desktop-native right-click menus.

Examples for conversation:

```text
Pin
Mute
Archive
Mark Read
Open in New Window
Delete
```

---

# 59. Message Context Menu

```text
Reply
Copy
Edit
Delete
Forward
Info
```

---

# 60. File Context Menu

```text
Open
Save As
Reveal in Folder
Forward
Copy Link
Delete Local Copy
```

---

# 61. Context Menu Rule

Every context-menu action should also be accessible via:

```text
keyboard
toolbar
menu
```

for accessibility.

---

# 62. Drag and Drop

Key desktop advantage.

Supported:

```text
file → conversation
conversation → tab
file → file destination
```

---

# 63. File Drop

Flow:

```text
OS file drop
    ↓
Dioxus platform event
    ↓
validated path/handle
    ↓
Rust file subsystem
    ↓
attachment draft
```

---

# 64. No UI Byte Loading

Dioxus should pass:

```text
file path / handle
```

not load full file into component state.

---

# 65. Drag Attachment Preview

Show:

```text
drop overlay
```

when user drags file over conversation.

---

# 66. Invalid Drop

If unsupported or too large:

```text
show inline rejection
```

not silent failure.

---

# 67. Clipboard

Support:

```text
copy text
copy image
paste image/file
paste text
```

---

# 68. Clipboard Security

Sensitive screens may restrict:

```text
recovery code
```

copy behavior via explicit action only.

---

# 69. Composer Focus

On opening conversation:

```text
do not always steal keyboard focus
```

unless user navigated to compose intentionally.

---

# 70. Global Focus Model

Track:

```text
active pane
focused element
modal
palette
```

---

# 71. Focus Restoration

Closing dialog/palette should return focus to previous meaningful element.

---

# 72. Focus Ring

Always visible for keyboard interaction.

---

# 73. Mouse/Keyboard Coexistence

Do not hide important actions only on hover.

---

# 74. Tooltips

Use for:

```text
icon-only toolbar buttons
shortcut hints
```

---

# 75. Toolbar

Conversation toolbar:

```text
Audio Call
Video Call
Search
Details
More
```

---

# 76. Call Toolbar

Active call:

```text
Mute
Video
Screen Share
Audio Route
More
Hang Up
```

---

# 77. File Toolbar

```text
Search
Sort
Filter
View Mode
```

---

# 78. Global Header

Optional top-level area for:

```text
back/forward
current context
global search
account status
window controls
```

---

# 79. Native Window Decorations

Two strategies:

```text
native OS title bar
custom app title bar
```

Recommendation:

```text
native where practical
```

unless custom chrome materially improves product.

---

# 80. Custom Chrome Risks

Must handle:

```text
drag region
window controls
accessibility
platform conventions
```

---

# 81. System Menu

Desktop app menu:

```text
File
Edit
View
Conversation
Call
Window
Help
```

where OS convention supports.

---

# 82. File Menu

Examples:

```text
New Conversation
Open File
Export
Quit
```

---

# 83. Edit Menu

```text
Undo
Redo
Cut
Copy
Paste
Select All
```

---

# 84. View Menu

```text
Toggle Sidebar
Toggle Inspector
Zoom
Full Screen
Compact Density
```

---

# 85. Conversation Menu

```text
Search
Mute
Pin
Archive
Details
```

---

# 86. Call Menu

```text
Mute
Video
Screen Share
End Call
```

---

# 87. Window Menu

```text
Minimize
New Window
Bring All to Front
```

platform-dependent.

---

# 88. Help Menu

```text
Keyboard Shortcuts
Diagnostics
Documentation
About
```

---

# 89. Dock/Taskbar Badge

Unread count can appear on app icon where platform supports.

Derived from durable unread projection.

---

# 90. Taskbar Progress

Optional for:

```text
long file transfer
```

Do not overuse.

---

# 91. System Notifications

Desktop notification click should navigate to:

```text
conversation
call
security event
```

using typed deep link.

---

# 92. Notification Suppression

If relevant window already focused:

```text
prefer in-app banner
```

---

# 93. In-App Banner

Desktop top-right or top-center ephemeral banner.

Use for:

```text
incoming message while viewing another screen
transfer completed
security warning
```

---

# 94. Call Overlay

When user is elsewhere:

```text
small persistent call bar
```

shows:

```text
participant
duration
mute
return to call
hang up
```

---

# 95. Persistent Call Bar

Visible across main destinations while call active.

---

# 96. Minimized Call

Possible compact floating window.

---

# 97. PiP-Like Desktop Video

Optional floating mini video.

---

# 98. Multi-Call

If call waiting later added:

```text
active call
held call
incoming call
```

shell must display clear priority.

---

# 99. Sidebar Search

Secondary sidebar can include scoped search.

Examples:

```text
filter chats
filter contacts
filter files
```

---

# 100. Global Search

Separate from sidebar filter.

Part 32 engine.

---

# 101. Search Result Pane

Could open:

```text
main workspace
```

with filters/results.

---

# 102. Search Preview

Wide desktop may show:

```text
results
+
preview pane
```

---

# 103. Jump-to-Message

Load conversation around stable `MessageId`.

---

# 104. Search History

Optional local-only.

---

# 105. Details Pane Architecture

Potential panels:

```text
Conversation Info
Shared Media
Members
Security
Files
Links
```

---

# 106. Details Pane Navigation

Within inspector, use compact tabs or sections.

---

# 107. Inspector Persistence

Persist:

```text
open/closed
width
last section per context optionally
```

---

# 108. Pane Resizing

User can drag divider.

Persist widths.

---

# 109. Minimum Pane Widths

Prevent unusable panes.

---

# 110. Collapsing Rule

When main workspace becomes too narrow:

```text
collapse inspector first
then secondary sidebar
```

---

# 111. Layout Priority

Priority order:

```text
main content
primary navigation
secondary list
inspector
```

---

# 112. Fullscreen Mode

Useful for:

```text
video call
media viewer
presentation
```

---

# 113. Zen Mode

Optional:

```text
hide rail/sidebar
focus conversation
```

---

# 114. New Conversation Flow

Desktop:

```text
Ctrl/Cmd+N
    ↓
search/select contact
    ↓
create/open conversation
```

---

# 115. Compose New Message Modal

Could be:

```text
center modal
or
main workspace
```

Recommendation:

```text
main workspace or lightweight modal
```

depending density.

---

# 116. Contact Search

Immediate local search.

No network dependency.

---

# 117. Unknown Peer Request

Clearly labeled.

---

# 118. Group Creation

Desktop wizard/panel:

```text
select members
name group
permissions
create
```

---

# 119. File Browser

Desktop can offer richer view modes:

```text
list
grid
details
```

---

# 120. Sort

Examples:

```text
Newest
Oldest
Name
Size
Type
```

---

# 121. Filters

```text
Conversation
Sender
Type
Date
```

---

# 122. Transfer Center

Dedicated destination or bottom utility panel.

Show:

```text
active uploads
active downloads
queued
failed
complete recent
```

---

# 123. Transfer Utility Panel

Optional collapsible bottom panel.

---

# 124. Bottom Status Area

Could expose:

```text
connection health
active transfer count
backup warning
call state
```

Keep minimal by default.

---

# 125. Status Bar

Advanced mode only if product needs.

Avoid overwhelming normal users.

---

# 126. Network Indicator

Use simple:

```text
Online
Offline
Reconnecting
```

Detailed path diagnostics live elsewhere.

---

# 127. Diagnostics Entry

Easy to reach from:

```text
Help
Settings
connection indicator context menu
```

---

# 128. Developer Mode

Can add:

```text
protocol inspector
path graph
event viewer
plugin console
```

without affecting normal UI.

---

# 129. Settings Desktop Layout

Recommended:

```text
settings sidebar
+
settings content
```

---

# 130. Settings Search

Useful for large settings set.

---

# 131. Settings Navigation

Typed pages:

```rust
pub enum SettingsPage {
    Account,
    PrivacySecurity,
    Notifications,
    CallsMedia,
    StorageData,
    Devices,
    Appearance,
    Plugins,
    Advanced,
    About,
}
```

---

# 132. Settings Persistence

UI writes commands to Rust/platform settings services.

---

# 133. Appearance

Desktop-specific:

```text
theme
density
font scale
sidebar width
window behavior
```

---

# 134. Theme

Support:

```text
System
Light
Dark
```

---

# 135. Accent Color

Optional product setting.

---

# 136. Font Scale

Independent desktop UI scaling.

---

# 137. Accessibility

Desktop must support:

```text
keyboard-only navigation
visible focus
screen reader semantics
high contrast
font scaling
reduced motion
```

---

# 138. Tab Order

Predictable:

```text
rail
sidebar
main content
inspector
```

within screen.

---

# 139. Landmark Semantics

Expose meaningful regions:

```text
navigation
conversation list
message timeline
composer
details
```

---

# 140. Screen Reader Message Row

Coherent announcement:

```text
Alice, 10:42 AM, Hello, delivered
```

---

# 141. Message Selection Accessibility

Selection state announced.

---

# 142. Context Menu Accessibility

Keyboard accessible via:

```text
Shift+F10
Menu key
```

where supported.

---

# 143. Reduced Motion

Disable/reduce:

```text
pane slide animations
typing dots
large transitions
```

---

# 144. High Contrast

Do not rely solely on subtle gray differences.

---

# 145. Focus on New Message

Do not move keyboard focus when new message arrives.

---

# 146. Live Region

Screen reader can announce:

```text
new message
```

without stealing focus.

---

# 147. Window Accessibility

Detached windows need descriptive titles.

Examples:

```text
Call with Alice
Photo Viewer
Network Diagnostics
```

---

# 148. Performance

Desktop shell should remain responsive with:

```text
100k+ messages
large contact list
many files
active call
background transfer
```

---

# 149. Virtualization Required

For:

```text
conversation list
message timeline
contacts
files
search
```

---

# 150. Do Not Render Hidden Panes Expensively

Collapsed inspector should not maintain heavy rendering subscriptions.

---

# 151. Subscription Scope

Examples:

```text
active conversation → message/presence stream
visible sidebar → conversation summaries
active call → call metrics
```

---

# 152. Background Tabs

Background tabs receive:

```text
low-frequency summary updates
```

not every high-frequency event.

---

# 153. Call Metrics Throttling

UI:

```text
1–10 Hz
```

depending metric.

Not packet-level.

---

# 154. Transfer Progress Throttling

UI:

```text
5–10 Hz
```

is sufficient.

---

# 155. Layout Recalculation

Avoid excessive global recomputation.

Use localized Dioxus signals.

---

# 156. Presenter Pattern

Each major desktop screen gets:

```text
Presenter
Snapshot
Event subscription
Commands
```

---

# 157. Example Chat Presenter

```rust
pub struct ChatPresenter {
    pub conversations: Signal<Vec<ConversationSummary>>,
    pub active: Signal<Option<ConversationScreenSnapshot>>,
}
```

---

# 158. Presenter Does Not Own Domain Truth

It maps core state into desktop presentation.

---

# 159. Daemon Mode

Desktop UI may run as separate process.

Architecture:

```text
Dioxus UI
    ↓
Local IPC Client
    ↓
comm-daemon
    ↓
Rust Core
```

---

# 160. IPC Semantics

Desktop UI service API remains same shape.

---

# 161. Daemon Disconnect

If UI loses daemon:

```text
show reconnecting
disable commands that require core
keep local shell usable
```

---

# 162. Daemon Reconnect

Re-request fresh snapshots.

Do not assume missed events are enough.

---

# 163. UI Process Restart

Daemon continues:

```text
call
transfer
sync
```

if architecture permits.

New UI attaches to snapshots.

---

# 164. Core Embedded Mode

Single-process desktop:

```text
Dioxus + Rust Core
```

same presentation API.

---

# 165. Runtime Abstraction

```rust
pub enum DesktopCoreMode {
    InProcess,
    LocalDaemon,
}
```

---

# 166. Mode Transparency

Screen code should not care.

---

# 167. Window Manager Service

```rust
pub trait DesktopWindowManager {
    fn open(&self, destination: DesktopDestination, kind: DesktopWindowKind);
    fn close(&self, id: WindowId);
}
```

---

# 168. Navigation Service

```rust
pub trait DesktopNavigator {
    fn navigate(&self, destination: DesktopDestination);
    fn back(&self);
    fn forward(&self);
}
```

---

# 169. Command Service

```rust
pub trait DesktopCommandRegistry {
    fn execute(&self, command: DesktopCommand);
}
```

---

# 170. Desktop Command

Examples:

```rust
pub enum DesktopCommand {
    NewConversation,
    GlobalSearch,
    ToggleSidebar,
    ToggleInspector,
    ToggleMute,
    ToggleVideo,
    OpenSettings,
    OpenDiagnostics,
}
```

---

# 171. Menu Integration

Menus invoke the same command registry.

---

# 172. Shortcut Integration

Shortcuts invoke the same command registry.

---

# 173. Palette Integration

Command palette invokes the same command registry.

---

# 174. Context Menu Integration

Where appropriate, same command model with entity context.

---

# 175. Unified Command Architecture

This prevents:

```text
same action implemented differently in menu, shortcut, toolbar
```

---

# 176. Undo System

Presentation/domain actions that support undo can return:

```text
UndoToken
```

---

# 177. Desktop Undo

Show:

```text
toast/banner with Undo
```

for:

```text
archive
mute
remove local download
```

---

# 178. No Fake Undo

Do not offer undo for:

```text
device revoke
identity reset
cryptographic erasure
```

unless backend truly supports reversal.

---

# 179. Onboarding Desktop

Initial shell should guide:

```text
create/link identity
add contact
link device
test notifications
```

Detailed onboarding comes later in UI/UX Part 25.

---

# 180. Empty Main Workspace

When nothing selected:

```text
welcome panel
recent activity
quick actions
```

---

# 181. Quick Actions

Examples:

```text
New Conversation
Add Contact
Link Device
Search
```

---

# 182. No Marketing Dashboard

Main empty workspace should remain functional/productive.

---

# 183. Account Switcher

If multi-account later supported:

```text
account menu in rail/header
```

---

# 184. Profile Menu

Contains:

```text
status
DND
devices
settings
lock app
quit
```

---

# 185. Presence Control

User can set:

```text
Online
Away
DND
Invisible
```

if product exposes manual presence.

---

# 186. Security State Indicator

Small account-level warning badge if:

```text
new device
identity issue
backup stale
```

---

# 187. Backup Health Indicator

Avoid constant icon if healthy.

Show only when action needed.

---

# 188. Storage Warning

Global banner when:

```text
disk nearly full
```

with action:

```text
Manage Storage
```

---

# 189. Update Notification

Desktop update architecture may later show:

```text
Update available
```

without blocking.

---

# 190. Release Notes

Optional modal/page.

---

# 191. Crash Recovery UX

On restart after crash:

```text
restore window state conservatively
resume daemon state
show recovery notice only if needed
```

---

# 192. Corrupt UI Layout State

If persisted layout corrupt:

```text
reset UI layout
```

without affecting core data.

---

# 193. Layout Reset

Settings action:

```text
Reset Window Layout
```

---

# 194. Workspace Persistence

Persist:

```text
rail expanded
sidebar width
inspector width
open destination
tabs
window state
```

---

# 195. Sensitive Persistence

Do not persist:

```text
recovery code open
security confirmation modal
temporary passwords
```

---

# 196. History Persistence

Navigation history may be session-only.

---

# 197. Sidebar State

Persist last selected top-level destination.

---

# 198. Conversation Draft

Handled by message draft feature, not generic shell state.

---

# 199. Desktop Deep-Link Examples

Potential URI scheme:

```text
comm://conversation/<id>
comm://call/<id>
comm://device/<id>
```

Exact scheme later.

---

# 200. Single Instance Handling

Opening a deep link when app already running:

```text
route to existing main instance
```

or open new window based on policy.

---

# 201. File Association

Optional for:

```text
backup archive
export archive
plugin package
```

---

# 202. File Open Flow

OS opens file:

```text
validate type
inspect safely
show import/install UI
```

---

# 203. Plugin Package Open

Route to Part 24 plugin manager.

---

# 204. Backup Archive Open

Route to restore inspector.

---

# 205. External Link Handling

URLs in messages open through:

```text
safe confirmation policy
default browser
```

---

# 206. Potential Link Preview

Preview is separate backend feature.

---

# 207. Security for External Links

Warn for suspicious schemes.

---

# 208. Desktop Notification Center

Optional app-internal notification history for:

```text
security
transfers
missed calls
```

Not necessarily all chat messages.

---

# 209. Utility Panel

Optional right/bottom utility area for:

```text
transfers
notifications
call
diagnostics
```

Keep collapsed by default.

---

# 210. Do Not Overload Shell

Avoid:

```text
5 permanent panels
30 icons
always-visible debug information
```

Normal experience should remain calm.

---

# 211. Visual Hierarchy

Priority:

```text
active conversation/content
navigation
contextual actions
secondary status
```

---

# 212. Density

Desktop can show more data, but preserve readable line lengths.

---

# 213. Message Width

Conversation bubbles/text should not stretch edge-to-edge on ultrawide monitor.

Use max readable width.

---

# 214. Centered Timeline

On wide screen:

```text
timeline centered
details pane optional
```

---

# 215. File Grid Width

Adaptive columns.

---

# 216. Contacts Layout

List or compact cards.

---

# 217. Accessibility + Density

Compact mode must not reduce hit targets below usability.

---

# 218. Theme Tokens

Desktop shell consumes shared design tokens from UI/UX Part 22 later.

---

# 219. Animation

Use subtle:

```text
pane transitions
selection
notification banner
```

Avoid mobile-like large sliding screens for every desktop navigation.

---

# 220. Reduced Motion

Disable nonessential transitions.

---

# 221. Startup Experience

Goal:

```text
window appears quickly
local shell renders
local conversation list loads
network attaches afterward
```

---

# 222. Splash

Avoid long custom splash.

---

# 223. Loading Core

If daemon/core not ready:

```text
show shell
"Connecting to local service…"
```

---

# 224. Fatal Core Error

Offer:

```text
Restart Service
Open Diagnostics
Restore Backup
Quit
```

---

# 225. Definition of Done

UI/UX Part 02 is complete when:

- the desktop app has a clear rail/sidebar/workspace architecture
- compact, medium, wide, and ultrawide layouts are defined
- pane collapse priorities are deterministic
- multi-window behavior is defined
- active calls can survive navigation/window changes
- system tray behavior is defined
- close/hide/quit semantics are unambiguous
- keyboard-first navigation is first-class
- command palette, menus, shortcuts, toolbar actions, and context menus share one command model
- drag-and-drop and clipboard flows route into Rust file/message services
- raw files/media do not live in Dioxus component state
- window and pane layout state persists independently of core data
- typed navigation/deep-link destinations are used
- Dioxus can run either in-process with the core or as a local-daemon client
- daemon reconnect uses fresh snapshots
- lists are virtualized
- high-frequency backend events are throttled before UI
- accessibility, focus restoration, keyboard navigation, screen-reader semantics, and reduced motion are explicit requirements
- the shell exposes all major backend systems without surfacing backend implementation complexity to ordinary users

---

# 226. Final Desktop Architecture

```text
                           MAIN WINDOW
+-----------------------------------------------------------------------+
| Header / Back / Forward / Global Search / Account                     |
+------+----------------------+--------------------------+---------------+
| Rail | Secondary Sidebar    | Main Workspace           | Inspector     |
|      |                      |                          |               |
| Chat | Conversation List    | Conversation             | Info          |
| Call | Call History         | Call                     | Media         |
| File | Files                | File Viewer              | Security      |
| ...  | ...                  | Search / Settings        | Members       |
+------+----------------------+--------------------------+---------------+
| Optional Utility / Transfer / Call Status                             |
+-----------------------------------------------------------------------+
```

Supporting windows:

```text
Call Window
Media Viewer
Diagnostics
```

Input surfaces:

```text
mouse
keyboard
drag/drop
clipboard
menus
command palette
tray
notifications
deep links
```

All actions converge on:

```text
Rust presentation commands
```

and all state comes from:

```text
Rust snapshots + events
```

---

# 227. Final Principle

The desktop app should behave like a native productivity workspace built around communication.

The right desktop model is:

```text
persistent navigation
+
large-screen information density
+
keyboard-first interaction
+
multi-pane context
+
optional multi-window workflows
+
shared Rust product truth
```

not:

```text
mobile screens scaled to desktop
```

Dioxus should therefore be used to build a desktop-specific shell that fully exploits desktop interaction while remaining a thin, consistent presentation client over the Rust core.
