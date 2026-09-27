# UI/UX Part 23 — Responsive / Adaptive Desktop, Tablet, Foldable & Phone Layout Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 23  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete responsive/adaptive layout architecture across desktop, tablet, foldable, and phone form factors, including window classes, pane composition, navigation adaptation, hinge/posture handling, orientation, multi-window, IME/system insets, state continuity, scroll/focus preservation, large-text reflow, and shared presentation contracts.

---

# 1. Purpose

The same product must work across:

```text
small phone
large phone
tablet
foldable
small desktop window
large desktop window
ultrawide desktop
split-screen
multi-window
```

without treating each as a separate product.

The governing principle is:

> **Product state is shared; layout is adaptive.**

A conversation remains the same conversation whether it is rendered in:

```text
one pane
two panes
three panes
separate windows
```

---

# 2. Architectural Position

```text
Rust Product State
        │
        ▼
Presentation Snapshot / Navigation State
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
   │          │
Responsive / Adaptive Layout Resolver
```

---

# 3. Responsive vs Adaptive

Responsive:

```text
sizes/spacing/content flow change continuously
```

Adaptive:

```text
layout structure changes between modes
```

This platform needs both.

---

# 4. Core Layout Modes

Recommended semantic modes:

```rust
pub enum AdaptiveLayoutMode {
    Compact,
    Medium,
    Expanded,
    Wide,
}
```

---

# 5. Platform Mapping

Android:

```text
Compact
Medium
Expanded
```

Desktop:

```text
Compact
Medium
Wide
UltraWide
```

Mapped into shared semantic intent where useful.

---

# 6. Window Class Is Not Device Type

Do not infer:

```text
phone == compact
tablet == expanded
desktop == wide
```

A tablet in split-screen may be compact.

A desktop window can be narrow.

---

# 7. Layout Resolver Inputs

```rust
pub struct LayoutEnvironment {
    pub width: f32,
    pub height: f32,
    pub orientation: Orientation,
    pub text_scale: f32,
    pub input_mode: InputMode,
    pub posture: DevicePosture,
    pub safe_insets: Insets,
}
```

---

# 8. Device Posture

```rust
pub enum DevicePosture {
    Flat,
    Book,
    Tabletop,
    Tent,
    Unknown,
}
```

Mostly relevant to foldables.

---

# 9. Input Mode

```rust
pub enum InputMode {
    Touch,
    PointerKeyboard,
    Mixed,
}
```

---

# 10. Layout Must Not Be Chosen from User-Agent Alone

Hard rule.

Use actual current window metrics/capabilities.

---

# 11. Desktop Primary Layout

Recommended default on wide desktop:

```text
Primary Rail
Secondary Sidebar
Main Workspace
Optional Inspector
```

---

# 12. Desktop Compact Window

When narrow:

```text
Rail
Main Workspace
```

Sidebar becomes:

```text
temporary drawer
overlay
or destination replacement
```

---

# 13. Desktop Medium

Recommended:

```text
Rail
Sidebar
Main
```

Inspector hidden by default.

---

# 14. Desktop Wide

Recommended:

```text
Rail
Sidebar
Main
Inspector optional
```

---

# 15. Desktop UltraWide

Can support:

```text
Rail
Sidebar
Main
Inspector
Utility pane
```

but avoid unused empty columns.

---

# 16. Main Workspace Max Width

Conversation content should remain readable.

Do not stretch message bubbles across ultrawide displays.

---

# 17. Inspector Width

Bounded.

Should not shrink main conversation below usable minimum.

---

# 18. Sidebar Min/Max Width

Use semantic limits.

---

# 19. Desktop Resizing

Window resize must be live.

No restart.

---

# 20. Pane Collapse Priority

Recommended collapse order:

```text
Utility
Inspector
Secondary Sidebar
Primary Rail labels
```

Do not collapse main workspace first.

---

# 21. Rail Behavior

Wide:

```text
icon + label
```

Narrow:

```text
icon
```

with accessible labels.

---

# 22. Sidebar Behavior

Can transform into:

```text
drawer
modal panel
route
```

depending width.

---

# 23. Inspector Behavior

Wide:

```text
persistent side pane
```

Narrow:

```text
dialog
sheet
separate route
```

---

# 24. Desktop Conversation Example

Wide:

```text
Chats list | Conversation | Details
```

Narrow:

```text
Conversation
```

with list reachable via navigation.

---

# 25. Android Phone Primary Layout

Compact:

```text
Top App Bar
Main Content
Bottom Navigation
```

---

# 26. Android Primary Destinations

Recommended:

```text
Chats
Calls
Contacts
```

Files/Search/Settings accessible through secondary routes as previously defined.

---

# 27. Phone Conversation Flow

```text
Chats
→ Conversation
→ Profile/Details
```

single-pane navigation.

---

# 28. Android Medium

Large phone/small tablet:

```text
navigation rail optional
list-detail where useful
```

---

# 29. Android Expanded

Tablet:

```text
Navigation Rail
List Pane
Detail Pane
```

---

# 30. Tablet Conversation

Recommended:

```text
Conversation List | Conversation
```

---

# 31. Tablet Details

Optional third pane only at sufficient width.

---

# 32. Android List-Detail Principle

Stable selection in list drives detail.

Do not use two unrelated navigation stacks.

---

# 33. No Selection on Expanded Layout

If nothing selected:

```text
empty detail state
```

Example:

```text
Select a conversation
```

---

# 34. Returning from Compact to Expanded

Restore previously selected item if still valid.

---

# 35. Expanded to Compact

Keep currently active detail as route.

Do not bounce user back to list unexpectedly.

---

# 36. Foldable Architecture

Foldables require posture-aware layout, not only width.

---

# 37. Book Posture

Recommended:

```text
List on one pane
Detail on other pane
```

if hinge separates usable regions.

---

# 38. Tabletop Posture

Potential:

```text
content upper half
controls/composer lower half
```

for calls or media where useful.

---

# 39. Hinge Avoidance

Never place:

```text
critical button
text field
message content
```

under physical hinge.

---

# 40. Hinge as Divider

Can become natural pane separator.

---

# 41. Foldable Conversation

Book posture:

```text
conversation list | conversation
```

---

# 42. Foldable Calls

Tabletop posture:

```text
remote video
---------------- hinge
call controls
```

---

# 43. Foldable Camera Preview

Avoid spanning hinge.

---

# 44. Orientation Changes

Portrait/landscape should not reset:

```text
selection
draft
scroll anchor
call state
search query
```

---

# 45. Android Configuration Change

ViewModel/Rust presentation survives.

Compose re-resolves layout.

---

# 46. Process Death

Separate from orientation.

Restore from durable Rust state + saved presentation state.

---

# 47. State Continuity

Shared presentation identity uses stable IDs:

```text
ConversationId
MessageId
ContactId/AccountId
BlobId
CallId
PluginId
```

---

# 48. Scroll Anchor

For message timeline:

```rust
pub struct ScrollAnchor {
    pub item: MessageId,
    pub offset: f32,
}
```

---

# 49. Layout Transition

When pane structure changes:

```text
preserve logical anchor
```

not raw scroll pixel if geometry changes.

---

# 50. Search Result Position

Preserve:

```text
SearchRequestId
result key
scroll position
```

across resize.

---

# 51. Focus Preservation

If focused item remains visible:

```text
keep focus
```

If its pane disappears:

```text
move focus to nearest logical control
```

---

# 52. Desktop Inspector Collapse

If focus was inside inspector and it collapses:

```text
move focus to inspector trigger
```

---

# 53. Android Pane Collapse

If detail pane becomes separate route:

```text
focus starts on detail heading/content
```

not bottom navigation.

---

# 54. Large Text Reflow

Text scale can force layout adaptation earlier than width alone.

---

# 55. Effective Width

Layout resolver may account for:

```text
window width
+
text scale
```

---

# 56. Large Text Desktop

Could collapse inspector even at same physical width.

---

# 57. Large Text Android

List-detail may become single-pane earlier.

---

# 58. IME / Software Keyboard

Composer screen must adapt without hiding:

```text
text field
send
attachment controls
```

---

# 59. Android IME Insets

Use platform insets.

---

# 60. Conversation Timeline During IME

Preserve bottom/anchor behavior.

---

# 61. Composer Expansion

Multi-line field grows up to bounded height.

---

# 62. IME + Attachment Panel

Avoid stacking too many vertical overlays.

---

# 63. Desktop Virtual Keyboard

If tablet/convertible OS keyboard appears, respect insets where available.

---

# 64. System Insets

Android:

```text
status bar
navigation bar
cutout
IME
```

Desktop:

```text
window chrome
safe areas
```

---

# 65. Edge-to-Edge Android

Supported, but critical controls remain inset-safe.

---

# 66. Landscape Phone

Potential:

```text
navigation rail
wider conversation
```

but do not force split panes on very low height.

---

# 67. Height Matters

A very wide but short window may need compact vertical behavior.

---

# 68. Minimum Height Rules

Calls/dialogs/settings should remain scrollable.

---

# 69. Desktop Small Height

Avoid fixed-height panes that hide controls.

---

# 70. Multi-Window Desktop

Possible windows:

```text
Main
Call
Media
Settings
Diagnostics
```

---

# 71. Shared Core State

All windows observe same Rust truth.

---

# 72. Window-Local Presentation State

Each window may own:

```text
scroll
selection
pane widths
focus
```

---

# 73. Conversation in Multiple Windows

If supported, each window has independent viewport/scroll but shared message truth.

---

# 74. Read Receipt Implication

Rust combines viewport/focus observations from all relevant windows.

---

# 75. Android Multi-Window

App may run in split-screen/freeform.

Treat as current window class.

---

# 76. Picture-in-Picture

Calls only.

PiP is a special adaptive surface.

---

# 77. PiP Call Layout

Minimal:

```text
video
mute state
return
hang up if supported
```

---

# 78. PiP State Continuity

Same CallId.

No new call state.

---

# 79. Desktop Floating Call Window

Likewise same CallId.

---

# 80. Layout Persistence

Desktop may persist:

```text
window size
position
sidebar width
inspector width
last layout mode
```

---

# 81. Persistence Safety

Recover from:

```text
removed monitor
resolution change
DPI change
```

---

# 82. Offscreen Recovery

Window must reopen on visible display.

---

# 83. Android Layout Persistence

Do not persist raw pixel widths.

Use semantic state:

```text
selected conversation
selected tab
pane visibility preference
```

---

# 84. User-Resizable Desktop Panes

Allow sidebar/inspector resizing.

---

# 85. Pane Constraints

Clamp to min/max.

---

# 86. Reset Layout

Settings/diagnostics action:

```text
Reset Layout
```

---

# 87. Pane Resize Accessibility

Keyboard-adjustable or not essential.

---

# 88. Splitter Accessibility

If splitter is focusable:

```text
Left/Right adjust
Home/End reset
```

---

# 89. Adaptive Navigation

Navigation changes form, not meaning.

---

# 90. Shared Destination Model

```rust
pub enum AppDestination {
    Chats,
    Conversation(ConversationId),
    Calls,
    Contacts,
    Files,
    Search,
    Devices,
    Security,
    Settings,
    Plugins,
    Diagnostics,
}
```

---

# 91. Desktop Mapping

```text
rail/sidebar/windows
```

---

# 92. Android Mapping

```text
bottom nav/rail/nav graph
```

---

# 93. Destination Stability

Layout transition must not mutate destination.

---

# 94. Deep Link Stability

Notification/deep link opens same semantic destination regardless of form factor.

---

# 95. Conversation Details Mapping

Wide desktop:

```text
inspector
```

Phone:

```text
separate route
```

Tablet:

```text
detail pane or route
```

---

# 96. Contact Profile Mapping

Same principle.

---

# 97. File Details Mapping

Wide:

```text
inspector
```

Compact:

```text
screen/sheet
```

---

# 98. Search Preview Mapping

Desktop wide:

```text
results + preview
```

Phone:

```text
results → destination
```

---

# 99. Security Center Mapping

Desktop:

```text
category + detail
```

Android compact:

```text
category list → detail screen
```

---

# 100. Settings Mapping

Same semantic categories, different navigation.

---

# 101. Plugin Manager Mapping

Desktop list/detail.

Android list/detail route.

---

# 102. Diagnostics Mapping

Desktop rich panes/tables.

Android stacked cards/screens.

---

# 103. Emergency Mapping

Always prioritize simple high-clarity full surface.

Do not compress SOS into narrow side pane during active emergency.

---

# 104. Call Mapping

Active call can break normal layout rules.

---

# 105. Call Fullscreen

Optional.

---

# 106. Call Split Screen

Wide desktop:

```text
call + chat/details
```

if user chooses.

---

# 107. Tablet Call

Video can dominate while chat panel appears side-by-side in expanded layout.

---

# 108. Phone Call

Single dominant call surface.

---

# 109. Media Viewer Mapping

Phone:

```text
full screen
```

Desktop:

```text
window/modal
```

Tablet:

```text
full/detail pane
```

---

# 110. Responsive Component Behavior

Components need adaptation rules.

---

# 111. Button Labels

Narrow layout may use icon-only for secondary actions.

Accessible labels remain.

---

# 112. Toolbar Overflow

Move low-priority actions into:

```text
More
```

---

# 113. Action Priority

Define:

```text
Primary
Secondary
Overflow
```

---

# 114. Composer Actions

Phone:

```text
attach + send visible
others in menu
```

Desktop wide:

```text
more actions visible
```

---

# 115. Conversation Header

Compact:

```text
back
title
call
more
```

Wide:

```text
title
presence
call/video/search/details
```

---

# 116. File Toolbar

Compact:

```text
search
filter
more
```

Wide:

```text
search
filter
sort
layout
details
```

---

# 117. Settings Row

Compact:

```text
stack description beneath title
```

Wide:

```text
title/description left
control right
```

---

# 118. Table to Cards

Diagnostics tables on narrow screens become stacked rows/cards.

---

# 119. Group Member Management

Phone:

```text
full screen
```

Desktop wide:

```text
detail pane/table
```

---

# 120. Dialog Adaptation

Compact Android:

```text
full-screen flow
```

for complex forms.

Desktop:

```text
modal dialog/window
```

---

# 121. Bottom Sheets

Phone contextual actions.

---

# 122. Side Sheets

Tablet/desktop optional.

---

# 123. Drawer

Use carefully.

Desktop sidebar collapse can use temporary drawer.

Android navigation drawer only if needed; primary nav already bottom/rail.

---

# 124. Content Reordering

Responsive layout may rearrange panels, but not meaning.

---

# 125. Semantic Reading Order

Accessibility order follows logical workflow, not visual x/y only.

---

# 126. Desktop Three-Pane Reading Order

Recommended:

```text
rail
sidebar
main
inspector
```

---

# 127. Foldable Reading Order

Book posture:

```text
left logical pane
right logical pane
```

adjust for RTL.

---

# 128. RTL Adaptation

Pane order mirrors where appropriate.

---

# 129. Conversation List in RTL

Sidebar can move to right depending platform convention.

---

# 130. Technical Content

IDs/code remain canonical direction.

---

# 131. Large Media

Use responsive aspect constraints.

---

# 132. Image Viewer

Maintain:

```text
fit
zoom
pan
```

regardless of width.

---

# 133. Video Controls

Never placed under hinge/inset.

---

# 134. Avatars

Scale within token limits.

Do not become huge on ultrawide.

---

# 135. Max Content Width

Important for:

```text
messages
settings prose
security explanations
backup warnings
```

---

# 136. Wide Empty Space

Use margins rather than stretching paragraphs.

---

# 137. Virtualized Lists

Responsive changes must preserve stable item keys.

---

# 138. Conversation List Virtualization

No full reinitialization on pane resize.

---

# 139. Timeline Virtualization

Preserve MessageId anchor.

---

# 140. File Grid

Column count changes with width.

---

# 141. Grid Reflow

Preserve selected item and approximate scroll anchor.

---

# 142. Search Result Layout

Compact:

```text
list
```

Wide:

```text
list + preview
```

---

# 143. Transfer Center

Compact:

```text
stacked cards
```

Wide:

```text
table/list
```

---

# 144. Responsive Empty States

Avoid huge illustrations on small windows.

---

# 145. Responsive Banners

Narrow:

```text
stacked
```

Wide:

```text
inline
```

---

# 146. Error Actions

Never disappear due to narrow width.

Move to overflow only if still discoverable.

---

# 147. Destructive Actions

Do not place in cramped toolbars.

Use detail/menu confirmation.

---

# 148. Window Class Resolver

Conceptual:

```rust
pub trait LayoutResolver {
    fn resolve(
        &self,
        env: LayoutEnvironment,
    ) -> AdaptiveLayoutPlan;
}
```

---

# 149. Adaptive Layout Plan

```rust
pub struct AdaptiveLayoutPlan {
    pub mode: AdaptiveLayoutMode,
    pub navigation: NavigationLayout,
    pub panes: PaneLayout,
    pub inspector: InspectorMode,
    pub density: DensityMode,
}
```

---

# 150. Navigation Layout

```rust
pub enum NavigationLayout {
    BottomBar,
    NavigationRail,
    PrimaryRail,
    Drawer,
}
```

---

# 151. Pane Layout

```rust
pub enum PaneLayout {
    Single,
    ListDetail,
    ListDetailInspector,
    Custom,
}
```

---

# 152. Inspector Mode

```rust
pub enum InspectorMode {
    Hidden,
    Overlay,
    Persistent,
    SeparateRoute,
}
```

---

# 153. Layout Policy Ownership

Platform UI resolves concrete layout.

Rust may expose semantic screen state.

---

# 154. Do Not Put Pixel Breakpoints in Domain Core

Hard rule.

---

# 155. Shared Presentation Contract

Rust should provide enough data for:

```text
list
detail
inspector
```

without requiring separate business queries when layout expands.

---

# 156. Example Conversation Snapshot

```rust
pub struct ConversationWorkspaceView {
    pub conversation: ConversationScreenSnapshot,
    pub details: Option<ConversationDetailsView>,
}
```

---

# 157. Lazy Detail Loading

Allowed.

If inspector opens, request details.

---

# 158. Avoid Overfetching

Compact phone need not load heavy inspector data.

---

# 159. Prefetch Strategy

Wide layout may prefetch adjacent detail safely.

---

# 160. Layout Event

```rust
pub enum LayoutUiEvent {
    WindowClassChanged(AdaptiveLayoutMode),
    PostureChanged(DevicePosture),
    InsetsChanged(Insets),
}
```

Platform-local event, not domain truth.

---

# 161. UI State Model

```rust
pub struct WorkspacePresentationState {
    pub destination: AppDestination,
    pub selected_secondary: Option<StableUiId>,
    pub selected_inspector: Option<StableUiId>,
    pub scroll_anchor: Option<StableScrollAnchor>,
}
```

---

# 162. Platform Owns Workspace Presentation State

Not domain core.

---

# 163. Save/Restore Presentation State

Android:

```text
SavedStateHandle for lightweight state
```

Desktop:

```text
local layout preferences
```

---

# 164. Durable Product State

Still Rust-owned.

---

# 165. Multi-Pane Navigation Rule

Do not treat opening inspector as new conversation destination unless platform needs route representation.

---

# 166. Browser-Like Back Stack

Android:

```text
back closes transient overlay
then detail
then list
```

according to semantic hierarchy.

---

# 167. Desktop Back/Forward

Optional history.

---

# 168. Escape Key Desktop

Close:

```text
menu
dialog
overlay
inspector if temporary
```

before changing destination.

---

# 169. Foldable Posture Change During Call

Same CallId.

Only surface layout changes.

---

# 170. Foldable Posture Change During Composer

Draft persists.

---

# 171. Hinge State Change During QR Scan

Scanner viewport reflows.

Session remains.

---

# 172. Orientation Change During Device Link

Session persists.

---

# 173. Orientation Change During Backup

Job persists.

---

# 174. Orientation Change During SOS

Emergency event persists.

---

# 175. Large Text During Active Screen

If user changes font scale, layout reflows without losing state.

---

# 176. Theme Change

No layout identity reset.

---

# 177. Density Change Desktop

Preserve:

```text
selection
scroll
focus where possible
```

---

# 178. Responsive Performance

Resize should not trigger expensive domain reloads.

---

# 179. Debounce

Continuous desktop resize can debounce expensive layout calculations.

Visual sizing still tracks smoothly.

---

# 180. No Network Work on Resize

Hard rule.

---

# 181. No Database Query Per Pixel Change

Hard rule.

---

# 182. Pane Data Subscription

Subscribe based on semantic pane visibility changes, not every width tick.

---

# 183. Example

Inspector hidden → visible:

```text
subscribe/load inspector data once
```

---

# 184. Split-Screen Android

Treat window changes as normal adaptive transition.

---

# 185. Desktop DPI Change

Recompute metrics/tokens.

---

# 186. Moving Window Between Monitors

Preserve layout state.

---

# 187. Window Min Size

Set reasonable minimum for desktop.

---

# 188. Below Minimum

If OS allows smaller, degrade to:

```text
single-pane
scrollable
```

rather than overlap.

---

# 189. Fullscreen Desktop

Can hide system chrome while preserving rail/sidebar as configured.

---

# 190. Kiosk/Managed Mode

Future.

May lock layout.

---

# 191. Accessibility Integration

Part 21 rules override density/layout choices.

---

# 192. Screen Reader

Pane transitions must be announced semantically.

---

# 193. Large Text

Can force mode change.

---

# 194. Keyboard

All panes reachable without pointer.

---

# 195. Switch Access

Single-pane phone remains fully sequential.

---

# 196. Color/Motion

Layout transitions do not rely on animation.

---

# 197. RTL

Adaptive pane order respects locale.

---

# 198. Testing Dimensions

Required:

```text
width
height
orientation
text scale
input mode
posture
theme
RTL
```

---

# 199. Desktop Test Matrix

```text
minimum window
compact
medium
wide
ultrawide
multi-monitor
DPI change
window restore offscreen
keyboard
```

---

# 200. Android Test Matrix

```text
small phone portrait
large phone portrait
phone landscape
tablet portrait
tablet landscape
split-screen
freeform
large text
TalkBack
```

---

# 201. Foldable Test Matrix

```text
flat
book
tabletop
hinge transition
orientation change
multi-window
```

---

# 202. Conversation Tests

Verify:

```text
selection preserved
draft preserved
scroll anchor preserved
unread separator preserved
```

across layout changes.

---

# 203. Search Tests

Query/result selection preserved.

---

# 204. Call Tests

Active call survives:

```text
orientation
PiP
fold
desktop window changes
```

---

# 205. File Tests

Grid/list reflow preserves selected file and approximate anchor.

---

# 206. Security Tests

Confirmation dialogs remain fully visible at smallest supported dimensions.

---

# 207. Backup Tests

Progress remains visible and job continues across layout changes.

---

# 208. Emergency Tests

SOS controls never disappear below fold/hinge.

---

# 209. Settings Tests

Two-pane → single-pane transition preserves selected category.

---

# 210. Plugin Tests

Plugin extension surfaces adapt through host layout rules.

---

# 211. Diagnostics Tests

Tables degrade to accessible stacked representation.

---

# 212. Performance Tests

Continuous desktop resize does not cause:

```text
network requests
database storms
large memory spikes
```

---

# 213. Accessibility Tests

At 200% text:

```text
no critical truncation
no overlapping panes
```

---

# 214. Screenshot/Golden Tests

Capture canonical form factors:

```text
phone compact
phone landscape
tablet expanded
foldable book
desktop compact
desktop wide
desktop ultrawide
```

---

# 215. State Transition Tests

Test:

```text
Compact → Expanded
Expanded → Compact
Flat → Book
Book → Tabletop
```

---

# 216. Navigation Regression Tests

Back behavior remains correct after adaptive transitions.

---

# 217. Input Regression Tests

IME opening/closing does not lose composer focus/draft.

---

# 218. Pane Resize Tests

User-resized sidebar survives restart within bounds.

---

# 219. Multi-Window Tests

Same data updates coherently across windows.

---

# 220. Initial Production Scope

Ship:

```text
desktop compact/medium/wide/ultrawide modes
desktop pane collapsing/resizing
Android compact/medium/expanded
phone bottom navigation
tablet rail + list-detail
foldable book/tabletop awareness
hinge-safe layout
orientation/multi-window support
IME/system inset handling
scroll/focus/state preservation
large-text adaptive collapse
responsive file grid/search/settings/diagnostics
```

Defer:

```text
arbitrary detachable panes
fully customizable dashboard layouts
complex window tiling system
per-screen user-authored responsive rules
```

---

# 221. Definition of Done

UI/UX Part 23 is complete when:

- layout is resolved from current window metrics/capabilities rather than device labels
- desktop supports compact, medium, wide, and ultrawide composition
- Android supports compact, medium, and expanded layouts
- pane collapse priority is explicit
- phone uses single-pane navigation while tablet/foldable can use list-detail
- foldable book/tabletop posture and hinge avoidance are defined
- orientation, split-screen, freeform, PiP, and desktop resizing do not reset product state
- message scroll anchors, selections, drafts, search state, and focus survive layout changes
- large-text scaling can trigger earlier layout collapse
- IME/system insets never hide critical composer/call controls
- multi-window shares Rust truth but keeps window-local presentation state
- responsive transitions do not trigger network/database storms
- deep links and semantic destinations remain stable across form factors
- accessibility reading order, keyboard navigation, RTL, and screen-reader semantics adapt with pane structure
- Dioxus and Compose retain platform-native navigation patterns while sharing destination/state semantics
- responsive/adaptive testing covers width, height, orientation, text scale, input mode, posture, theme, and RTL
- core flows remain usable at minimum supported size and 200% text

---

# 222. Final Architecture

```text
                      PRODUCT STATE
                          │
                          ▼
                 Presentation Snapshot
                          │
            ┌─────────────┴─────────────┐
            │                           │
        Dioxus                      Compose
        Desktop                     Android
            │                           │
     Window Metrics              Window Metrics
     Pointer/Keyboard            Touch/Posture/IME
            │                           │
            ▼                           ▼
       Adaptive Plan               Adaptive Plan
            │                           │
   Rail/Sidebar/Main           BottomNav/Rail/Panes
   Inspector/Windows           Phone/Tablet/Foldable
```

State continuity:

```text
same stable IDs
+
same destination
+
same Rust truth
+
layout-local presentation state
=
seamless form-factor transition
```

---

# 223. Final Principle

Responsive design should not mean shrinking the desktop UI until it fits a phone.

The correct model is:

```text
shared product state
+
shared semantic destinations
+
adaptive pane composition
+
platform-native navigation
+
stable continuity across transitions
```

not:

```text
one fixed layout stretched across every screen size
```

This gives Dioxus desktop and Android Compose a coherent product experience across phones, tablets, foldables, small windows, multi-window environments, and ultrawide desktops without duplicating business logic or losing user context.
