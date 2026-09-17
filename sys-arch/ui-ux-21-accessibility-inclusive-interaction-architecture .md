# UI/UX Part 21 — Accessibility & Inclusive Interaction Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 21  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete accessibility and inclusive-interaction architecture across messaging, calls, files, search, pairing, notifications, security, backup, emergency, settings, plugins, diagnostics, and all future product surfaces.

---

# 1. Purpose

Accessibility must be treated as a product architecture concern, not a collection of post-release fixes.

The platform must remain usable for people who may have:

```text
low vision
blindness
color-vision deficiency
hearing loss
motor impairments
tremor
limited dexterity
temporary injury
cognitive load constraints
attention limitations
reading difficulty
speech limitations
photosensitivity
one-handed use requirements
language or RTL needs
situational impairments
```

The governing principle is:

> **Every core task must be possible without relying on a single sense, gesture, input device, color, animation, or visual layout assumption.**

---

# 2. Architectural Position

```text
Rust Product Semantics
        │
        ├── action meaning
        ├── state meaning
        ├── severity
        ├── labels
        ├── capabilities
        └── accessibility metadata
        │
        ▼
Accessibility-Aware Presentation Layer
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
        │
        ▼
Platform Accessibility APIs
```

Accessibility is not an isolated module.

It crosses:

```text
domain semantics
presentation contracts
navigation
component design
testing
localization
platform adapters
```

---

# 3. Accessibility Product Goals

The product should support:

```text
screen-reader-first interaction
keyboard-only desktop use
switch-access compatibility
large-text layouts
high-contrast use
reduced motion
audio-independent understanding
color-independent understanding
gesture-independent alternatives
clear error recovery
simple critical flows
```

---

# 4. Core Accessibility Rule

Never make a task depend solely on:

```text
color
hover
drag
swipe
long press
pinch
sound
vibration
animation
spatial position
tiny target
timed reaction
```

Always provide an equivalent alternative.

---

# 5. Semantic UI Model

The presentation layer should expose meaning, not only visual styling.

Example:

```rust
pub struct AccessibleActionView {
    pub label: String,
    pub description: Option<String>,
    pub state: Option<AccessibleState>,
    pub importance: AccessibleImportance,
}
```

---

# 6. Accessible State

```rust
pub enum AccessibleState {
    Selected,
    Checked,
    Disabled,
    Expanded,
    Collapsed,
    Busy,
    Error,
    Warning,
}
```

---

# 7. Accessible Importance

```rust
pub enum AccessibleImportance {
    Normal,
    Important,
    Critical,
}
```

---

# 8. Dioxus Semantics

Desktop components must expose:

```text
role
name
state
description
keyboard behavior
focusability
```

through platform-appropriate accessibility APIs.

---

# 9. Compose Semantics

Compose components must use:

```text
semantics
contentDescription where appropriate
stateDescription
role
liveRegion
heading
collectionInfo
```

only when needed.

---

# 10. Do Not Over-Label

Bad:

```text
Button, Send button, button
```

Good:

```text
Send message
```

---

# 11. Visible Labels Preferred

Whenever possible, visible text should also be the accessible name.

---

# 12. Icon-Only Controls

Must have explicit label.

Examples:

```text
Search
Mute microphone
Attach file
Close
```

---

# 13. Ambiguous Icons

Avoid icon-only actions where meaning is non-obvious.

---

# 14. Screen Reader Navigation

Major screens should expose meaningful regions:

```text
navigation
header
main content
composer
details/inspector
status
```

---

# 15. Headings

Use semantic headings for:

```text
screen title
section title
security warning
settings category
```

---

# 16. Lists

Conversation lists, contacts, files, plugins, and settings should expose:

```text
collection semantics
item position if helpful
selected state
```

---

# 17. Message Timeline Semantics

Each message should expose:

```text
sender
time
message body
attachment summary
delivery state for own message
reply context if relevant
```

---

# 18. Avoid Screen Reader Noise in Timeline

Do not announce:

```text
avatar image
bubble background
decorative timestamp separators
```

as independent focusable items.

---

# 19. Message Grouping Accessibility

Grouped messages may visually omit repeated sender names.

Screen reader output should still preserve sender context.

---

# 20. Reply Accessibility

Example:

```text
Replying to Alice: "See you tomorrow"
```

---

# 21. Reaction Accessibility

Example:

```text
2 thumbs up reactions
```

with accessible details action.

---

# 22. Delivery Accessibility

Own message:

```text
Delivered
Read
Failed to send
```

must be available as text semantics.

---

# 23. Typing Accessibility

Announce:

```text
Alice is typing
```

only when meaningful.

Do not repeat every keepalive.

---

# 24. Presence Accessibility

Example:

```text
Alice, online
```

or:

```text
Alice, status unavailable
```

---

# 25. Composer Accessibility

Required elements:

```text
message field
reply context
attachment list
voice note action
send action
validation error
```

---

# 26. Composer Error

Screen reader should hear:

```text
Message cannot be sent: attachment is too large
```

---

# 27. Attachment Accessibility

Example:

```text
PDF, report.pdf, 2.4 megabytes, ready to send
```

---

# 28. Voice Recording Accessibility

Must support explicit buttons:

```text
Start recording
Pause
Resume
Stop
Cancel
Send
```

Do not rely only on hold gesture.

---

# 29. Audio Playback Accessibility

Expose:

```text
Play
Pause
Seek
Playback speed
Duration
Current position
```

---

# 30. Video Accessibility

Support:

```text
captions/subtitles when available
playback controls
audio description track if supported
```

---

# 31. Calls Accessibility

Incoming call surface must expose:

```text
caller
audio/video type
Accept
Decline
Silence
```

---

# 32. Active Call Accessibility

Expose:

```text
Mute microphone
Turn camera off
Switch camera
Audio route
End call
Screen share
```

with state.

---

# 33. Call State Description

Examples:

```text
Microphone muted
Camera on
Using Bluetooth headset
Connection reconnecting
```

---

# 34. Call Quality Accessibility

Do not rely on bars/color.

Use:

```text
Connection good
Connection poor
Reconnecting
```

---

# 35. Captions

If live captions are implemented later:

```text
captions should be accessible to screen readers
adjustable size
high contrast
position configurable
```

---

# 36. Search Accessibility

Search result must include:

```text
type
source
match context
```

Example:

```text
Message from Alice in Project Group, yesterday
```

---

# 37. Search Highlight Accessibility

Visual highlight is supplemental.

Snippet remains fully readable.

---

# 38. File Accessibility

File item example:

```text
Image, vacation.jpg, downloaded
```

or:

```text
PDF, report.pdf, downloading, 42 percent
```

---

# 39. Transfer Progress Accessibility

Do not announce every percentage.

Announce:

```text
Started
25%
50%
75%
Complete
Failed
```

or only on focus.

---

# 40. QR Accessibility

QR must never be the only path.

Provide:

```text
manual code
copy code
SAS
NFC if available
```

---

# 41. QR Scanner Accessibility

Include:

```text
instructions
close action
manual code alternative
```

Do not require precise camera aiming without alternative.

---

# 42. SAS Accessibility

Use:

```text
large grouped digits or words
```

Screen reader should announce groups clearly.

---

# 43. Security Accessibility

Security alerts must expose:

```text
severity
what happened
what action is needed
```

---

# 44. Recovery Key Accessibility

Provide:

```text
show
hide
copy
grouped reading
```

Avoid impossible visual-only key verification.

---

# 45. Backup Accessibility

Expose:

```text
backup state
last backup date
verification state
progress
failure
```

---

# 46. Emergency Accessibility

SOS activation must be possible without:

```text
press-and-hold only
```

Provide explicit confirmation flow.

---

# 47. Emergency Status Accessibility

Example:

```text
SOS active. Delivered to 2 of 3 contacts. One acknowledgement.
```

---

# 48. Emergency Location Accessibility

Example:

```text
Precise location shared
```

or:

```text
Location not shared
```

---

# 49. Settings Accessibility

Each setting exposes:

```text
title
description
current value
scope
managed state
```

---

# 50. Managed Setting

Example:

```text
Read receipts, off, managed by organization
```

---

# 51. Plugin Accessibility

Plugins must not be allowed to create inaccessible extension surfaces.

---

# 52. Plugin UI Contract

Every plugin-provided element must include:

```text
accessible name
role
state
focus behavior
```

---

# 53. Inaccessible Plugin Extension

Host validation should:

```text
reject
disable
or flag
```

missing semantics.

---

# 54. Diagnostics Accessibility

Diagnostic health:

```text
Network degraded. Relay available. Direct connection unavailable.
```

---

# 55. Graph Accessibility

Any chart must have:

```text
text summary
key values
trend description
```

---

# 56. Keyboard-Only Desktop UX

Every primary action must be reachable by keyboard.

---

# 57. Keyboard Focus Order

Follow logical reading order.

Do not follow arbitrary DOM/component creation order.

---

# 58. Focus Rings

Always visible for keyboard navigation.

---

# 59. Do Not Remove Focus Outline

Hard rule unless replaced with equally visible custom treatment.

---

# 60. Skip Navigation

For complex desktop layouts, support:

```text
skip sidebar
skip to main content
skip to composer
```

through shortcuts/focus commands.

---

# 61. Primary Keyboard Navigation

Recommended:

```text
Tab / Shift+Tab
Arrow keys
Enter
Space
Escape
```

---

# 62. Global Shortcuts

Examples:

```text
Ctrl/Cmd+K command palette
Ctrl/Cmd+Shift+F search
Ctrl/Cmd+F current scope search
```

Must never be the only access path.

---

# 63. Shortcut Discovery

Show in:

```text
menus
tooltips
command palette
keyboard shortcuts help
```

---

# 64. Shortcut Customization

Future optional.

Avoid conflicts with assistive technologies.

---

# 65. Context Menu Access

Mouse right-click must have keyboard equivalent:

```text
Shift+F10
Menu key
dedicated More Actions button
```

---

# 66. Hover Actions

Never make essential actions hover-only.

---

# 67. Drag and Drop

Always provide:

```text
Choose File
Move Up/Down
Add Attachment
```

alternatives.

---

# 68. Swipe Actions

Android swipe-to-archive/delete must have:

```text
menu/button
```

alternative.

---

# 69. Long Press

Must not be sole access to:

```text
selection
message actions
contact actions
```

---

# 70. Gesture Alternatives

Pinch zoom:

```text
Zoom in
Zoom out
Reset
```

controls available where relevant.

---

# 71. Motor Accessibility

Touch targets should be generous.

Recommended minimum:

```text
48 x 48 dp Android
```

and equivalent comfortable desktop targets.

---

# 72. Target Spacing

Avoid tightly packed destructive/confirm actions.

---

# 73. Tremor Safety

Destructive actions should not sit immediately adjacent to frequent actions without separation.

---

# 74. Repeat Click Protection

High-risk actions use:

```text
idempotent command
disabled while committing
```

---

# 75. Double Activation

Repeated Enter/tap should not duplicate:

```text
send
device revoke
SOS
backup restore
```

---

# 76. Switch Access

UI should work with sequential focus traversal.

---

# 77. Voice Access

Visible control labels help platform voice-command systems.

Avoid five controls all labeled:

```text
More
```

without contextual names.

---

# 78. Control Naming

Prefer:

```text
More message actions
More contact actions
More call actions
```

---

# 79. One-Handed Android UX

Primary frequent controls should be within comfortable reach.

Examples:

```text
composer
send
call controls
navigation
```

---

# 80. Bottom-Sheet Ergonomics

Use bottom sheets for secondary actions on phone.

Avoid placing every frequent action at top-right.

---

# 81. Foldables

Do not require reaching across hinge/large span for critical paired controls.

---

# 82. Large Text

Support OS font scaling.

Do not cap below platform accessibility expectations.

---

# 83. Large Text Layout Rule

Rows should grow vertically.

Do not:

```text
clip
overlap
truncate critical text
```

---

# 84. Minimum Responsive Text Assumption

Design should remain usable at:

```text
200% text scaling
```

where practical.

---

# 85. Compact Desktop Mode

Must still preserve accessible minimum targets and text.

---

# 86. Information Density

Compact mode is optional.

Accessibility defaults should remain comfortable.

---

# 87. Text Truncation

Only truncate non-critical secondary text.

Always allow access to full value via:

```text
details
tooltip
expand
```

---

# 88. Security Warnings

Never truncate consequences.

---

# 89. Error Messages

Never truncate required recovery steps.

---

# 90. Color Accessibility

No state may depend on color alone.

---

# 91. Status Combination

Use:

```text
icon
text
shape
```

in addition to color.

---

# 92. Contrast

Use WCAG-aligned contrast targets.

Recommended baseline:

```text
4.5:1 normal text
3:1 large text / essential UI graphics
```

where applicable.

---

# 93. Disabled Controls

Still readable.

Do not use extremely low contrast.

---

# 94. Focus Contrast

Keyboard focus indication must stand out against both light/dark themes.

---

# 95. High-Contrast Mode

Respect platform/system high-contrast where available.

Optional app override:

```text
High Contrast
```

---

# 96. Color-Vision Deficiency

Do not pair:

```text
green = good
red = bad
```

without icon/text.

---

# 97. Presence Dots

Pair with semantic label/tooltip.

---

# 98. Transfer State

Use:

```text
Downloading
Failed
Complete
```

not only color.

---

# 99. Security State

Use:

```text
Healthy
Warning
Critical
```

text.

---

# 100. Motion Accessibility

Respect:

```text
prefers-reduced-motion
Android animator duration scale / accessibility preferences where applicable
```

---

# 101. Reduced Motion Behavior

Disable/reduce:

```text
parallax
spring movement
large navigation transitions
pulsing status
animated path maps
```

---

# 102. Essential Motion

If motion conveys essential status, provide static equivalent.

---

# 103. Typing Indicator

Can become:

```text
Alice is typing…
```

without animated dots.

---

# 104. Progress Indicators

Use determinate/indeterminate static semantics.

---

# 105. No Flashing

Avoid flashing content above safe thresholds.

---

# 106. Emergency UX

Do not use strobe-like flashing.

---

# 107. Audio Accessibility

Every sound-only event must have visual or haptic equivalent.

---

# 108. Incoming Call

Use:

```text
visual notification
sound
optional vibration
```

not sound only.

---

# 109. Emergency Alert

Likewise.

---

# 110. Transfer Completion

Visual notification available.

---

# 111. Hearing Accessibility

Calls should support:

```text
captions later
visual mute state
audio route labels
text chat alongside call if product supports
```

---

# 112. Call Ringing

Visual incoming-call surface required.

---

# 113. Audio Alerts

Allow:

```text
sound off
vibration on
visual only
```

according to OS.

---

# 114. Haptics

Supplemental only.

---

# 115. Cognitive Accessibility

Critical flows should minimize working-memory demands.

---

# 116. Progressive Disclosure

Show:

```text
essential decision first
advanced detail later
```

---

# 117. Security Center

Normal users see:

```text
what happened
what to do
```

not raw key IDs.

---

# 118. Diagnostics

Normal users see:

```text
relay available
```

not protocol counters.

---

# 119. Backup Restore

Use step-by-step:

```text
inspect
plan
review
confirm
restore
```

---

# 120. Emergency

Minimize options during active crisis.

---

# 121. Clear Language

Prefer:

```text
Remove from this device
```

over:

```text
purge local blob
```

---

# 122. Consistent Terminology

Use the same words everywhere:

```text
Delivered
Read
Linked device
Verified contact
Backup
Restore
```

---

# 123. Avoid Synonym Drift

Do not alternate:

```text
Delete Local
Remove Device Copy
Clear File
```

for same operation.

---

# 124. Confirmation Design

Explain:

```text
what will happen
what will not happen
whether reversible
```

---

# 125. Timed Interactions

Avoid requiring response within short countdowns.

---

# 126. Exception: SOS Countdown

Must have:

```text
clear cancel
accessible announcement
optional configurable duration
```

---

# 127. Session Expiry

For QR/device linking, expiry should not require frantic action.

Offer fresh code easily.

---

# 128. Error Recovery

Every failure should expose:

```text
what failed
whether data is safe
what to do next
```

---

# 129. Composer Failure

Keep draft/content.

---

# 130. Transfer Failure

Offer retry.

---

# 131. Backup Failure

Do not delete last good backup.

---

# 132. Security Failure

Do not silently continue insecurely.

---

# 133. Undo

Use for safe reversible actions:

```text
archive conversation
mute
remove local copy
```

where practical.

---

# 134. No Undo for Security Illusion

Do not pretend:

```text
device revocation
identity reset
```

is easily undoable if it is not.

---

# 135. Localization

All user-facing text must be localizable.

---

# 136. No String Concatenation Assumptions

Avoid:

```text
"Alice" + " is typing"
```

where grammar differs by language.

---

# 137. Plurals

Use proper plural localization.

---

# 138. Dates/Times

Use locale-aware formatting.

---

# 139. Number Formatting

Use locale-aware:

```text
file sizes
counts
percentages
```

---

# 140. RTL

Support:

```text
Arabic
Urdu
Hebrew
```

layout mirroring.

---

# 141. Bidi Content

Messages can contain mixed:

```text
English + Urdu + IDs + URLs
```

Use robust bidi handling.

---

# 142. Technical Identifiers

Keep canonical direction.

Examples:

```text
fingerprints
DeviceId
URLs
codes
```

---

# 143. QR/SAS Codes

Never reorder due to RTL layout.

---

# 144. Message Bubble Alignment

Should follow product design, but text direction follows content/language.

---

# 145. Search Highlight

Must not break bidi text.

---

# 146. Internationalized Names

Do not assume:

```text
first name / last name
Latin characters
fixed length
```

---

# 147. Emoji

Support accessible naming where platform provides.

---

# 148. Avatar Fallback

Do not derive only from Latin initials.

Use robust Unicode grapheme handling.

---

# 149. Keyboard Input / IME

Support:

```text
CJK IMEs
Hindi transliteration
Urdu/Arabic keyboards
emoji
voice typing
```

---

# 150. Composer Send Shortcut

Must not break IME composition.

---

# 151. Enter During IME

Do not send while composition is active.

---

# 152. Selection

Text selection works with assistive technologies.

---

# 153. Clipboard

Do not block accessible copy unnecessarily.

Sensitive screens can still use explicit security restrictions.

---

# 154. Voice Input

Android voice typing should work naturally in composer.

---

# 155. Speech-to-Text

Platform capability.

Do not require custom implementation for basic input.

---

# 156. Reading Order

Visual multi-pane layouts must define logical accessibility order.

---

# 157. Desktop Three-Pane Layout

Recommended screen-reader order:

```text
primary navigation
conversation list
main content
inspector
```

unless focus is already inside main content.

---

# 158. Android List/Detail

TalkBack focus should not jump unpredictably when detail pane updates.

---

# 159. Focus Preservation

Incremental events should preserve focused semantic item.

---

# 160. Stable Identity

Use:

```text
MessageId
ConversationId
BlobId
AccountId
```

to preserve focus.

---

# 161. Dynamic Updates

Do not steal focus when:

```text
new message arrives
presence changes
transfer progresses
plugin updates
```

---

# 162. New Message While Reading History

Do not move screen-reader focus.

---

# 163. Live Regions

Use sparingly.

Good:

```text
Call disconnected
SOS acknowledged
```

Bad:

```text
every transfer percentage
every presence heartbeat
```

---

# 164. Priority Live Announcements

Critical:

```text
security warning
incoming call
SOS state
```

may need assertive announcement.

---

# 165. Notification Accessibility

OS notifications must have descriptive:

```text
title
body
actions
```

---

# 166. Lock-Screen Privacy

Accessibility cannot leak hidden notification content.

The strict privacy policy still applies.

---

# 167. Screen Magnification

Layouts must remain usable under zoom/magnifier.

Avoid fixed overlays covering important controls.

---

# 168. Desktop Zoom

Optional app UI scaling in addition to OS scale.

---

# 169. Android Magnification

Use standard Compose/layout behavior.

---

# 170. Touch Exploration

Do not hide controls until touch.

---

# 171. Pointer Precision

Desktop controls should not require tiny pixel targets.

---

# 172. Scrollbars

Desktop scrollbars should remain usable.

---

# 173. Virtualized Lists

Must preserve accessibility traversal.

---

# 174. Off-Screen Items

Do not expose thousands of virtualized off-screen nodes at once.

---

# 175. Pagination

Screen reader should know when more results load.

---

# 176. Infinite Scroll

Provide semantic announcement:

```text
20 more results loaded
```

only when useful.

---

# 177. Tables

Diagnostics/settings comparison tables need row/column semantics.

---

# 178. Complex Grids

Prefer lists over grids when accessibility is better.

---

# 179. Media Gallery

Grid semantics should expose:

```text
item
position
type
date
download state
```

---

# 180. Context Menus

Focus returns to invoking control after close.

---

# 181. Dialogs

Focus trapped within modal while open.

On close:

```text
return to logical trigger
```

---

# 182. Destructive Confirmation

Initial focus should not land on destructive action by default.

---

# 183. Bottom Sheets

Android should expose proper modal semantics.

---

# 184. Toasts/Snackbars

Important action errors must be accessible and persistent enough to hear/read.

---

# 185. Snackbar Actions

Examples:

```text
Retry
Undo
```

focusable.

---

# 186. Error Fields

Validation error associated with field.

---

# 187. Form Labels

Placeholder is not label.

---

# 188. Password/Secret Fields

Expose:

```text
Show/Hide
```

and proper secure-entry semantics.

---

# 189. Recovery Key Input

Allow paste and accessible grouping.

---

# 190. Progress Semantics

Every long operation needs:

```text
name
phase
state
progress when meaningful
cancel/retry if available
```

---

# 191. Indeterminate Progress

Screen reader:

```text
Preparing backup
```

not endless repeated spinner announcements.

---

# 192. Loading Skeletons

Decorative only.

Do not expose every skeleton row.

---

# 193. Empty States

Describe:

```text
what is empty
what user can do
```

Example:

```text
No contacts yet. Add a contact by QR, nearby, or invite code.
```

---

# 194. Offline States

Explain capability:

```text
No Internet. Nearby messaging is still available.
```

---

# 195. Error Tone

Avoid blame.

Use factual recovery language.

---

# 196. Inclusive Safety Language

Avoid assuming:

```text
all users can see
all users can hear
all users can hold device steadily
all users use mouse/touch
```

---

# 197. Inclusive Content Labels

Instead of:

```text
Click the red button
```

use:

```text
Select Delete
```

---

# 198. Platform Accessibility Settings Integration

Respect system:

```text
font scale
display scale
screen reader
reduced motion
high contrast where available
caption preferences
```

---

# 199. App Accessibility Settings

Only add overrides that solve real gaps.

---

# 200. Accessibility Settings Snapshot

```rust
pub struct AccessibilitySettingsView {
    pub reduce_motion: SettingControlState<bool>,
    pub high_contrast: SettingControlState<bool>,
    pub always_show_labels: SettingControlState<bool>,
    pub enhanced_delivery_text: SettingControlState<bool>,
    pub larger_controls: SettingControlState<bool>,
}
```

---

# 201. Presentation Metadata

A shared optional model:

```rust
pub struct AccessibilityMeta {
    pub label: Option<String>,
    pub hint: Option<String>,
    pub live: AccessibleLiveRegion,
    pub importance: AccessibleImportance,
}
```

---

# 202. Live Region

```rust
pub enum AccessibleLiveRegion {
    None,
    Polite,
    Assertive,
}
```

---

# 203. Do Not Move All Accessibility Strings into Rust

Platform/localization layer may generate natural labels.

Rust should expose semantic state necessary to produce them.

---

# 204. Rust Owns Semantic Truth

Examples:

```text
message is delivered
call is reconnecting
device is revoked
SOS acknowledged
```

---

# 205. UI Owns Natural Localized Description

Example:

```text
Delivered
```

or localized equivalent.

---

# 206. Accessibility Capability API

```rust
pub trait AccessibilityPresentation {
    async fn settings(
        &self,
    ) -> Result<AccessibilitySettingsView, UiError>;

    async fn update(
        &self,
        update: AccessibilitySettingsUpdate,
    ) -> Result<(), UiError>;

    async fn platform_capabilities(
        &self,
    ) -> Result<PlatformAccessibilityCapabilities, UiError>;
}
```

---

# 207. Platform Accessibility Capabilities

```rust
pub struct PlatformAccessibilityCapabilities {
    pub screen_reader_active: Option<bool>,
    pub reduce_motion_enabled: Option<bool>,
    pub high_contrast_enabled: Option<bool>,
    pub font_scale: Option<f32>,
}
```

Only expose what platform safely provides.

---

# 208. Screen Reader Detection

Do not use detection to remove functionality.

Use only for:

```text
minor optimization
better announcements
```

---

# 209. Never Create Separate "Accessible UI"

One UI should work for everyone.

---

# 210. Testing Strategy

Accessibility testing must combine:

```text
automated checks
semantic tree inspection
keyboard testing
screen-reader testing
large-font testing
RTL testing
manual task-based testing
```

---

# 211. Automated Checks

Use platform-appropriate tooling for:

```text
missing labels
contrast
touch target size
semantic roles
focusability
```

---

# 212. Automated Tests Are Not Enough

They cannot fully test:

```text
reading order
meaning
cognitive clarity
screen-reader usability
```

---

# 213. Desktop Screen Reader Testing

Test with supported platform combinations.

Examples:

```text
Linux accessibility stack
Windows Narrator/NVDA
macOS VoiceOver
```

as platform support matures.

---

# 214. Android Screen Reader Testing

Primary:

```text
TalkBack
```

---

# 215. Keyboard Test Matrix

Every release should test:

```text
launch
navigate inbox
open conversation
send message
search
open settings
manage file
answer/decline call
open security center
```

keyboard-only on desktop.

---

# 216. TalkBack Test Matrix

Test:

```text
open app
navigate chats
read message
reply
send attachment
answer call
search
link device via manual code
manage settings
```

---

# 217. Large Text Test Matrix

At least:

```text
default
130%
160%
200%
```

or platform equivalents.

---

# 218. RTL Test Matrix

At least:

```text
Arabic/Urdu UI
mixed English technical content
mixed message direction
```

---

# 219. Color Tests

Simulate:

```text
deuteranopia
protanopia
tritanopia
grayscale
```

for critical states.

---

# 220. Reduced Motion Tests

All core tasks remain understandable without animation.

---

# 221. Low-Vision Tests

Test:

```text
high contrast
large text
magnification
focus visibility
```

---

# 222. Motor Accessibility Tests

Verify:

```text
no essential drag
no essential swipe
no essential long press
reasonable target sizes
```

---

# 223. Cognitive Load Tests

Critical flows:

```text
device revoke
backup restore
SOS
identity reset
```

must be stepwise and explicit.

---

# 224. Accessibility Regression Tests

Create reusable scenario fixtures.

---

# 225. Screenshot Tests

Include:

```text
large text
RTL
high contrast
reduced motion
```

where screenshot testing applies.

---

# 226. Semantic Snapshot Tests

For core components, snapshot semantic tree.

---

# 227. Component Library Gate

No component enters shared design system without:

```text
keyboard behavior
screen-reader label strategy
focus behavior
large-text behavior
RTL behavior
contrast verification
```

---

# 228. Plugin Accessibility Gate

Plugins using declarative UI must pass host validation.

---

# 229. Accessibility Failure Severity

Critical failures include:

```text
cannot send message
cannot answer call
cannot cancel SOS
cannot revoke device
cannot restore backup
```

with assistive technology.

---

# 230. Release Quality Gate

Block release for accessibility regression in core task.

---

# 231. Accessibility Bug Taxonomy

```rust
pub enum AccessibilityIssueKind {
    MissingLabel,
    WrongRole,
    BrokenFocusOrder,
    KeyboardTrap,
    InvisibleFocus,
    LowContrast,
    TinyTarget,
    GestureOnly,
    MotionOnly,
    SoundOnly,
    TruncatedCriticalText,
    RtlBroken,
    ScreenReaderNoise,
}
```

---

# 232. Accessibility Telemetry

Do not collect disability status.

Hard rule.

---

# 233. Safe Product Metrics

Can measure generic:

```text
keyboard navigation errors
focus-loss bugs
layout overflow
```

without identifying assistive-tech users.

---

# 234. Screen Reader Usage Analytics

Avoid by default.

---

# 235. Crash Reports

No need to include accessibility user status unless necessary and privacy-safe.

---

# 236. Documentation

Provide:

```text
Keyboard Shortcuts
Accessibility
Screen Reader Support
Known Platform Limitations
```

---

# 237. Known Limitations

Be honest.

Example:

```text
Live call captions are not yet available.
```

---

# 238. Accessibility Help

Settings → Accessibility.

Include:

```text
keyboard shortcuts
motion
contrast
screen-reader notes
contact/support
```

---

# 239. Inclusive Onboarding

Do not force tutorials requiring gesture-only interactions.

---

# 240. Tutorial Controls

Provide:

```text
Skip
Next
Back
```

and keyboard/screen-reader semantics.

---

# 241. Notification Permission Education

Explain in plain language.

---

# 242. Camera/Nearby Permission Education

Offer alternatives.

---

# 243. QR Setup

Manual code path always visible.

---

# 244. Emergency Setup

Test mode accessible.

---

# 245. Security Setup

Recovery key can be copied/read without visual scanning.

---

# 246. Accessibility & Performance

Accessibility metadata should not cause large performance regressions.

---

# 247. Virtualization

Expose only visible semantic nodes while preserving logical navigation.

---

# 248. Large Lists

Focus should remain stable as pages load.

---

# 249. Incremental Updates

Presence/progress changes should not rebuild entire semantic tree.

---

# 250. Screen Reader Event Rate

Rate-limit non-critical announcements.

---

# 251. Accessibility & Privacy

Accessibility labels must obey privacy.

---

# 252. Lock Screen

Hidden sender/body stays hidden from accessibility service exposure where platform supports secure behavior.

---

# 253. App Lock

Protected content should not remain in accessibility tree after lock.

---

# 254. Sensitive Screens

Recovery keys, fingerprints, security secrets need deliberate semantics.

---

# 255. Screen Capture

Accessibility should not require disabling secure-window policy.

---

# 256. Clipboard Alternatives

If sensitive copy blocked, offer:

```text
read grouped key
save securely
```

alternative.

---

# 257. Accessibility & Security Tradeoffs

Do not weaken security unnecessarily.

Find accessible equivalents instead.

---

# 258. Example: Device Link

If camera inaccessible:

```text
manual code
```

not:

```text
disable verification
```

---

# 259. Example: Recovery

If visual QR inaccessible:

```text
manual recovery key
```

not:

```text
skip proof
```

---

# 260. Example: SOS

If hold gesture inaccessible:

```text
confirm button
```

not:

```text
single accidental tap
```

---

# 261. Desktop Platform Differences

Dioxus must adapt to:

```text
Windows accessibility
macOS accessibility
Linux AT-SPI/accessibility stack
```

through platform/runtime support.

---

# 262. Linux Accessibility

Test explicitly because desktop accessibility stacks vary.

---

# 263. Android Platform Differences

Compose integrates with Android semantics/TalkBack.

Use native semantics rather than custom gesture-heavy canvases.

---

# 264. Custom Drawing

If using custom canvas:

```text
provide virtual accessibility nodes
```

or avoid for interactive core controls.

---

# 265. Media Waveforms

Treat waveform as decorative unless interactive.

If seeking via waveform:

```text
provide slider semantics
```

---

# 266. Network Path Diagram

Decorative summary plus text:

```text
Connected through relay
```

---

# 267. QR Code

Decorative/encoded object plus manual equivalent.

---

# 268. Charts

Never sole source of diagnostic meaning.

---

# 269. Accessibility Design Tokens

Shared conceptual tokens should include:

```text
minimum target size
focus ring thickness
text scale behavior
contrast roles
motion durations
reduced-motion alternatives
```

---

# 270. Design Token Example

```rust
pub struct AccessibilityDesignTokens {
    pub min_touch_target_dp: f32,
    pub min_desktop_target_px: f32,
    pub focus_ring_width: f32,
    pub reduced_motion_duration_ms: u32,
}
```

---

# 271. Platform Rendering

Actual values can differ per platform while preserving intent.

---

# 272. Focus Token

Use consistent focus visual across desktop.

---

# 273. Error Token

Error must combine:

```text
icon
text
contrast
```

---

# 274. Warning Token

Likewise.

---

# 275. Critical Token

Used sparingly.

---

# 276. Screen Reader Copy Strategy

Accessible labels should be concise.

Long explanations belong in descriptions/details.

---

# 277. Duplicate Content

Avoid reading same text twice via:

```text
visible label
contentDescription
```

---

# 278. Decorative Images

Mark decorative.

---

# 279. Avatar Semantics

If identity already stated:

```text
avatar may be decorative
```

unless image carries distinct information.

---

# 280. Verification Icon

Needs semantic:

```text
Verified
Identity changed
Unverified
```

---

# 281. Status Icon

Needs semantic state.

---

# 282. Badge Count

Expose:

```text
3 unread messages
```

not only visual badge.

---

# 283. Unread Separator

Screen reader:

```text
Unread messages
```

---

# 284. Date Separator

Can announce:

```text
Today
Yesterday
August 24
```

when traversing.

---

# 285. Message Actions

More Actions menu lists:

```text
Reply
Copy
Forward
Delete
Details
```

with unavailable actions omitted/disabled semantically.

---

# 286. Group Member Roles

Screen reader:

```text
Alice, admin
```

---

# 287. Security Events

Severity announced first when critical.

---

# 288. Plugin Attribution

Screen reader:

```text
Translate using Translator Plugin
```

---

# 289. Diagnostic Result

Confidence wording remains accessible:

```text
Likely cause
```

---

# 290. Final Accessibility Presentation Boundary

```text
Rust:
    semantic truth
    capability
    state
    severity
    stable IDs

Presentation:
    accessible wording
    focus behavior
    input alternatives
    localized semantics

Platform:
    screen reader bridge
    keyboard/touch/switch support
    high contrast
    text scaling
    reduced motion
```

---

# 291. Initial Production Scope

Ship accessibility support for:

```text
TalkBack on Android
keyboard-only desktop
screen-reader semantic labels
large text
RTL
color-independent states
reduced motion
gesture alternatives
focus preservation
accessible messaging
accessible calls
accessible files/search
accessible security/recovery
accessible SOS
accessible settings
accessible plugin host surfaces
```

Defer only advanced enhancements such as:

```text
live captions
audio descriptions
custom shortcut remapping
specialized switch scanning optimization
```

if not yet implemented.

---

# 292. Definition of Done

UI/UX Part 21 is complete when:

- every core task has a non-visual and non-gesture-only path
- Android core flows are usable with TalkBack
- desktop core flows are usable by keyboard alone
- keyboard focus is visible, logical, and preserved across incremental updates
- hover, swipe, long-press, drag, pinch, sound, vibration, color, and animation are never the sole mechanism for essential actions
- message, call, file, search, pairing, security, backup, emergency, settings, plugin, and diagnostic surfaces have defined accessibility semantics
- large text does not truncate critical information
- RTL/bidi behavior is explicitly supported
- color contrast and color-independence rules are defined
- reduced-motion behavior is built into the design system
- accessibility labels obey privacy/security constraints
- recovery, device linking, and SOS flows remain secure without visual-only interaction
- plugin-provided UI must pass accessibility validation
- virtualized lists maintain stable semantic focus and bounded accessibility trees
- Rust supplies semantic truth while platform UI supplies localized accessible wording and interaction
- automated accessibility checks are supplemented by keyboard, TalkBack/screen-reader, large-text, RTL, color, motor, and cognitive task testing
- core accessibility regressions block release

---

# 293. Final Architecture

```text
                   PRODUCT SEMANTIC STATE
                             │
                             ▼
                  Accessibility-Aware
                 Presentation Contracts
                             │
          ┌──────────────────┼──────────────────┐
          │                                     │
       Dioxus                                Compose
       Desktop                               Android
          │                                     │
   Keyboard / Screen Reader               TalkBack / Switch
   Focus / High Contrast                  Font Scale / Motion
          │                                     │
          └──────────────────┬──────────────────┘
                             │
                     Same Product Truth
```

---

# 294. Final Principle

Accessibility should not create a second, simplified version of the application.

The correct model is:

```text
one product
+
one semantic truth
+
multiple equivalent interaction paths
+
platform-native accessibility
+
continuous testing
```

not:

```text
build the visual product first
→ patch accessibility afterward
```

This ensures the Dioxus desktop and Android Compose applications remain usable across different abilities, environments, input methods, and assistive technologies without weakening the security or local-first architecture.
