# UI/UX Part 04 — Conversation List / Inbox UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 04  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete conversation list / inbox experience across desktop and Android, including unread state, sorting, pinning, archiving, requests, presence, drafts, typing hints, mute state, delivery state previews, multi-device consistency, offline behavior, pagination, accessibility, context actions, and navigation into conversations.

---

# 1. Purpose

The conversation list is the primary operational surface of the application.

It must answer, at a glance:

```text
Who contacted me?
What is unread?
What changed recently?
Who is active?
Which messages failed?
Which conversations are pinned?
Which are muted?
Are there message requests?
Is something waiting to send?
```

The inbox must remain useful when:

```text
offline
partially synced
multi-device
background-updated
search rebuilding
peer unavailable
conversation has draft
conversation has pending transfer
```

The governing principle is:

> **The inbox is a projection of durable conversation truth, enriched with bounded ephemeral hints. It is never an independent message database.**

---

# 2. Architectural Position

```text
Rust Conversation Projection
        │
        ├── unread/read state
        ├── last durable message
        ├── draft summary
        ├── pin/mute/archive state
        ├── peer/group identity
        └── delivery/outbox state
        │
        + Ephemeral overlay
            ├── presence
            ├── typing
            └── call availability
        │
        ▼
ConversationSummary DTO
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Conversation Summary

Recommended shared presentation DTO:

```rust
pub struct ConversationSummary {
    pub id: ConversationId,
    pub kind: ConversationKind,
    pub title: String,
    pub avatar: AvatarRef,
    pub preview: ConversationPreview,
    pub timestamp: Timestamp,
    pub unread_count: u32,
    pub mention_count: u32,
    pub pinned: bool,
    pub muted: bool,
    pub archived: bool,
    pub draft: Option<DraftPreview>,
    pub send_state: ConversationSendState,
    pub presence: Option<PresenceSummary>,
    pub typing: Option<TypingSummary>,
    pub security_badge: Option<SecurityBadge>,
}
```

---

# 4. Conversation Kinds

```rust
pub enum ConversationKind {
    Direct,
    Group,
    MessageRequest,
    System,
}
```

Optional future:

```text
Channel
Broadcast
Organization
```

---

# 5. Inbox Sections

Recommended logical sections:

```text
Pinned
Recent
Requests
Archived
```

Do not create excessive permanent sections.

---

# 6. Default Main Inbox

Normal inbox contains:

```text
Pinned
Recent
```

Requests and archived conversations are separate destinations/filter states.

---

# 7. Pinned Conversations

Pinned conversations appear before normal recency-sorted conversations.

Within pinned:

```text
user-defined order
or
recent order
```

Recommendation:

```text
user-defined order
```

if drag-reordering is supported.

---

# 8. Pin Limit

Keep a reasonable product limit or allow unlimited but virtualized.

Do not make pinning affect durable protocol behavior.

It is user presentation metadata.

---

# 9. Recency Ordering

Recent conversations sort by meaningful activity:

```text
latest message
call event
draft change optionally
security event only if conversation-relevant
```

Do not reorder constantly because presence changes.

---

# 10. Presence Must Not Reorder Inbox

Example:

```text
Alice becomes online
```

should not jump conversation to top.

Presence is an overlay, not durable activity.

---

# 11. Typing Must Not Reorder Inbox

Likewise:

```text
Bob is typing…
```

should not move conversation position.

---

# 12. Drafts

A conversation with local draft may show:

```text
Draft: unfinished text
```

in preview.

---

# 13. Draft Priority

Preview priority example:

```text
typing
draft
failed outgoing
last durable message
```

But avoid unstable flickering.

Recommended display rules are defined explicitly below.

---

# 14. Preview State Types

```rust
pub enum ConversationPreview {
    Message(MessagePreview),
    Draft(DraftPreview),
    Typing(TypingPreview),
    Transfer(TransferPreview),
    Call(CallPreview),
    System(SystemPreview),
}
```

---

# 15. Stable Preview Priority

Recommended:

```text
1. typing/recording when fresh
2. draft if user has unsent draft
3. failed/pending outgoing state if important
4. latest durable message/call/file event
```

---

# 16. Typing Preview

Example:

```text
Alice is typing…
```

Group:

```text
Alice and Bob are typing…
```

If many:

```text
Several people are typing…
```

---

# 17. Typing TTL

UI never owns expiry.

Rust Part 30 projection sends:

```text
typing active
typing inactive
```

based on TTL.

---

# 18. Voice Recording Preview

Optional:

```text
Recording a voice message…
```

same ephemeral rules.

---

# 19. Last Message Preview

Possible:

```text
text excerpt
Photo
Video
Voice message
File
Missed call
```

Use human-readable semantic labels.

---

# 20. Message Preview Privacy

Inbox is inside app, but app lock/security profile may require:

```text
hide message previews
```

optionally.

---

# 21. Unknown Content

If message encrypted but not yet decryptable:

```text
New encrypted message
```

or generic:

```text
New message
```

not raw error.

---

# 22. Failed Outgoing Preview

Example:

```text
Failed to send
```

with warning indicator.

---

# 23. Queued Outgoing Preview

Example:

```text
Waiting for connection
```

or small clock icon.

---

# 24. Delivery State in Inbox

Do not overcrowd each row.

Possible only for user's latest outgoing message:

```text
Queued
Sent
Delivered
Read
Failed
```

represented compactly.

---

# 25. Unread Count

Derived from durable read cursor.

Never maintained separately by UI.

---

# 26. Mention Count

Group conversations may show:

```text
@2
```

or special badge when unread mentions exist.

---

# 27. Unread Badge

Normal:

```text
count
```

If count large:

```text
99+
```

or product-defined cap.

---

# 28. Muted Unread

Muted conversation still shows unread count.

Badge may be visually quieter.

---

# 29. Mark Read

Action advances durable read cursor through current eligible message sequence.

---

# 30. Mark Unread

If supported, this is a user reminder state.

Important distinction:

```text
read receipt already sent
```

cannot be undone remotely.

Local "mark unread" means:

```text
local reminder/unread marker
```

not reversing read history.

---

# 31. Local Unread Marker

Could store:

```rust
pub struct ManualUnreadMarker {
    pub conversation: ConversationId,
    pub from_seq: MessageSequence,
}
```

---

# 32. Archive

Archive removes conversation from normal inbox but does not delete it.

---

# 33. New Message in Archived Conversation

Policy options:

```text
unarchive automatically
or
remain archived
```

Recommended default:

```text
unarchive on new incoming message
```

unless muted/archived intentionally with special setting.

---

# 34. Delete Conversation

This must distinguish:

```text
delete local history
leave group
delete for everyone
```

Do not combine into one ambiguous action.

---

# 35. Direct Conversation Delete

Possible semantics:

```text
remove local conversation history
```

without deleting peer's copy.

---

# 36. Group Delete

Usually:

```text
clear local history
```

separate from:

```text
Leave group
```

---

# 37. Conversation Requests

Unknown peers should not enter normal inbox equivalently to trusted contacts.

Use:

```text
Requests
```

---

# 38. Request Row

Display:

```text
claimed display name
verification/trust state
message preview limited
request timestamp
```

---

# 39. Unknown Sender Safety

Avoid auto-loading:

```text
large attachment
link preview
external media
```

from unknown sender.

---

# 40. Request Actions

```text
Accept
Block
Delete
Report
```

where supported.

---

# 41. Accept Request

May:

```text
create trusted/accepted conversation relationship
```

but should not imply cryptographic verification.

---

# 42. Verification vs Acceptance

Display separately:

```text
Accepted
Verified
```

They are not identical.

---

# 43. Requests Badge

Main Chats destination can show:

```text
3 requests
```

without mixing requests into normal unread count if product chooses.

---

# 44. Search / Filter Inbox

Inbox filter field can search:

```text
conversation title
contact
group
```

This is lightweight list filtering.

Global message search remains Part 32.

---

# 45. Inbox Search Scope

Do not query all message history on every character in sidebar filter.

Filter loaded conversation summaries.

---

# 46. Advanced Filters

Potential:

```text
Unread
Groups
Direct
Muted
Archived
```

Use chips/menu, not permanent clutter.

---

# 47. Sort Options

Default:

```text
Pinned + Recent
```

Optional future:

```text
Unread first
Name
```

Avoid too many modes.

---

# 48. Conversation Row Anatomy

Recommended semantic structure:

```text
Avatar
Title
Timestamp
Preview
Unread/Mention Badge
Mute/Pin/Send Status
Presence optional
```

---

# 49. Avatar

Direct:

```text
contact avatar
```

Group:

```text
group avatar
```

Fallback:

```text
initials / generated identicon
```

---

# 50. Avatar Security

Do not imply verified identity through avatar appearance alone.

Use explicit verification marker if needed.

---

# 51. Presence Indicator

Small optional dot/status.

Do not show per-device presence.

---

# 52. Presence States

Recommended visible distinction:

```text
Online
Away
none
```

Avoid strong "Offline" dot if status is actually Unknown.

---

# 53. Timestamp

Use compact relative formatting:

```text
10:42
Yesterday
Mon
12 Aug
```

according to recency.

UI formats locally.

---

# 54. Timestamp Source

Use latest meaningful durable conversation activity.

Typing/presence should not alter timestamp.

---

# 55. Security Badge

Only when meaningful:

```text
identity changed
new unverified device
verification warning
```

Do not place green lock icon on every row.

---

# 56. Draft Badge

Preview prefix:

```text
Draft:
```

with distinct semantic styling.

---

# 57. Failed State

Latest outgoing failure can show:

```text
warning icon
Failed to send
```

---

# 58. Transfer State

If latest event is active file transfer:

```text
Sending file…
```

but transfer center remains authoritative.

---

# 59. Active Call State

Conversation row may show:

```text
Call in progress
```

if user is currently in call with that conversation.

---

# 60. Desktop Dioxus Inbox Layout

Recommended:

```text
+--------------------------------+
| Chats                    New + |
| Search / Filter                 |
+--------------------------------+
| Pinned                          |
|  Alice                    10:42 |
|  Team                     09:10 |
+--------------------------------+
| Recent                          |
|  Bob                      08:31 |
|  School ERP               Mon   |
|  ...                            |
+--------------------------------+
```

---

# 61. Desktop Sidebar Width

Allow resizing.

Minimum prevents title/preview becoming unusable.

---

# 62. Desktop Row Density

Potential modes:

```text
Comfortable
Compact
```

---

# 63. Desktop Hover Actions

On hover:

```text
Mute
Pin
More
```

but actions must remain available through context menu/keyboard.

---

# 64. Desktop Context Menu

```text
Open
Open in New Tab
Open in New Window
Pin/Unpin
Mute/Unmute
Mark Read/Unread
Archive
Delete
```

---

# 65. Desktop Keyboard Navigation

```text
Up/Down → move selection
Enter → open
Shift+F10 → context menu
Ctrl/Cmd+N → new conversation
```

---

# 66. Desktop Selection Model

Single active row.

Optional multi-select only if batch conversation operations are valuable.

Not necessary initially.

---

# 67. Desktop Active Row

Clearly distinguish:

```text
selected
unread
hovered
focused
```

without relying solely on color.

---

# 68. Desktop Split Pane

Clicking conversation updates main workspace while sidebar remains visible.

---

# 69. New Conversation Button

Visible in chats header.

Opens:

```text
contact picker
```

or command palette flow.

---

# 70. Android Compose Inbox Layout

Recommended:

```text
TopAppBar
    Chats
    Search
    New conversation / overflow

Optional filter chips

LazyColumn conversation list

BottomNavigation
```

---

# 71. Android Conversation Row

Use full-width touch row.

Information:

```text
avatar
title
preview
timestamp
unread badge
small state icons
```

---

# 72. Android Swipe Actions

Potential:

```text
swipe right → mark read/unread
swipe left → archive
```

only if discoverable and reversible.

---

# 73. Swipe Accessibility

Every swipe action must also exist in:

```text
long-press action sheet
```

---

# 74. Android Long Press

Opens conversation action sheet:

```text
Pin
Mute
Mark Read
Archive
Delete
```

---

# 75. Android FAB

Optional:

```text
New conversation
```

A FAB is reasonable if central to product.

Alternative:

```text
top app bar action
```

---

# 76. Recommendation

Use a small compose/new-message FAB on phone if it does not obscure content.

Tablet can use toolbar action.

---

# 77. Pull to Refresh

Optional.

If implemented:

```text
request sync
```

not clear/reload local state.

---

# 78. Offline Pull Refresh

May attempt:

```text
local discovery
outbox retry
sync
```

but retains list.

---

# 79. Tablet Inbox

Expanded layout:

```text
NavigationRail
    │
Conversation List
    │
Conversation Detail
```

---

# 80. Tablet Selection

Opening a row updates detail pane without replacing list.

---

# 81. Empty Detail Pane

If no conversation selected:

```text
Start a conversation
```

or welcome placeholder.

---

# 82. Foldable

If hinge separates:

```text
list on one region
detail on other
```

where ergonomic.

---

# 83. List Pagination

Conversation lists can be large.

Use cursor/page from Rust projection.

---

# 84. Initial Page

Load most recent:

```text
50–100
```

depending row cost and backend.

Tune by measurement.

---

# 85. Load More

At scroll threshold:

```text
request next page
```

---

# 86. Pinned Conversations

Pinned items can be loaded separately to guarantee visibility.

---

# 87. Stable Keys

Always:

```text
ConversationId
```

---

# 88. List Update

If message arrives:

```text
update row
move conversation according to durable recency
```

---

# 89. Move Animation

Use subtle position animation.

Respect reduced motion.

---

# 90. Avoid Row Teleport Confusion

If many messages arrive:

```text
batch/coalesce list reorders
```

rather than animate dozens of shifts.

---

# 91. Multi-Device Updates

If another device:

```text
reads
archives
pins
mutes
```

policy determines which settings sync.

---

# 92. Read State Sync

Durable read cursor syncs.

Inbox badge updates.

---

# 93. Pin Sync

Product decision:

```text
account-wide
or
device-local
```

Recommendation:

```text
account-wide
```

for consistency.

---

# 94. Mute Sync

Recommendation:

```text
account-wide semantic mute
```

OS notification-channel details remain device-local.

---

# 95. Archive Sync

Recommendation:

```text
account-wide
```

unless product defines device-local inbox organization.

---

# 96. Draft Sync

Can start:

```text
device-local
```

and evolve to account-wide later.

---

# 97. Presence Overlay

Presence is not stored into conversation row persistence.

UI receives updated overlay.

---

# 98. Typing Overlay

Typing similarly ephemeral.

---

# 99. Row Update Frequency

Presence/typing should only recompose the affected row.

Do not recompute whole list.

---

# 100. Compose State Granularity

Prefer stable list data with per-row identifiers.

Avoid giant mutable map causing full list recomposition.

---

# 101. Dioxus State Granularity

Same principle.

Use localized signals/subscriptions.

---

# 102. Inbox Snapshot

```rust
pub struct InboxSnapshot {
    pub pinned: Vec<ConversationSummary>,
    pub recent: Vec<ConversationSummary>,
    pub request_count: u32,
    pub archived_count: u32,
    pub next_cursor: Option<ConversationCursor>,
}
```

---

# 103. Inbox Events

```rust
pub enum InboxEvent {
    ConversationInserted(ConversationSummary),
    ConversationUpdated(ConversationSummary),
    ConversationRemoved(ConversationId),
    ConversationReordered {
        id: ConversationId,
        position: usize,
    },
    RequestCountChanged(u32),
}
```

---

# 104. Event Strategy

Can simplify:

```text
row upsert
+
sort key
```

and let presentation layer maintain ordering.

---

# 105. Conversation Sort Key

```rust
pub struct ConversationSortKey {
    pub pinned_rank: Option<u32>,
    pub last_activity: Timestamp,
}
```

---

# 106. Optimistic Archive

When user archives:

```text
Rust accepts command
→ row projection changes
→ UI animates out
```

If command fails:

```text
row returns
show snackbar
```

---

# 107. Undo Archive

Good use of snackbar:

```text
Conversation archived — Undo
```

---

# 108. Optimistic Mute

Can update after Rust confirms quickly.

---

# 109. Pin Reordering

Desktop drag.

Android long-press drag if worth complexity.

---

# 110. Recommendation for v1

Pinned ordering:

```text
automatic order based on pin timestamp
```

or stable simple order.

Manual pin reorder can be added later.

---

# 111. Message Request Security

Unknown requests must not trigger:

```text
presence sharing
read receipts
typing indicators
```

until accepted, according to privacy policy.

---

# 112. Read Receipt for Request

Do not send read receipt automatically to unknown sender by default.

---

# 113. Request Preview Privacy

Could show only:

```text
New message request
```

until user opens.

---

# 114. Spam Flood

If many unknown requests:

```text
collapse into request count
```

not hundreds of normal rows/notifications.

---

# 115. Request Rate Limit

Part 28 handles backend quotas.

UI does not render more than bounded page.

---

# 116. Blocked Conversations

Blocked peers should normally disappear from main inbox or appear in:

```text
Blocked
```

management area.

---

# 117. System Conversations

If product has:

```text
security/system notices
```

consider dedicated system area instead of impersonating a human conversation.

---

# 118. Conversation Creation

A new conversation may exist locally before first successful message.

---

# 119. Empty New Conversation

If user selects contact and opens composer without sending:

```text
do not necessarily persist inbox row
```

until:

```text
draft stored
or
first message sent
```

---

# 120. Draft-Only Conversation

If durable drafts enabled:

```text
show row
```

with:

```text
Draft
```

---

# 121. Conversation Deletion Race

If open conversation deleted on another device:

```text
remove from list
main pane shows no longer available
```

---

# 122. Group Rename

Inbox row title/avatar updates.

---

# 123. Contact Rename

Local alias update immediately changes row.

---

# 124. Identity Change

Security badge appears.

Do not silently replace verified identity without warning.

---

# 125. Outbox Failure

If latest outgoing message permanently failed:

```text
warning icon
```

and conversation remains visible.

---

# 126. Pending Queue

If offline:

```text
small clock
```

not scary red error.

---

# 127. Offline Global State

Could show:

```text
Offline
```

in chats header as subtle banner/chip.

Do not repeat "offline" on every row.

---

# 128. Reconnecting

Likewise global.

---

# 129. Row Presence While Offline

Remote presence becomes:

```text
Unknown
```

after TTL.

---

# 130. Inbox Search During Offline

Local filtering works fully.

---

# 131. Conversation Open During Offline

Full local history opens.

---

# 132. Accessibility — Row Semantics

Screen reader row could announce:

```text
Alice, 3 unread messages, last message "See you tomorrow", 10:42 AM, muted
```

---

# 133. Avoid Duplicate Semantics

Do not expose avatar/title/preview as many noisy separate nodes unless interaction benefits.

---

# 134. Android TalkBack

Whole row should be one meaningful clickable target with custom actions where useful.

---

# 135. Desktop Screen Reader

Focused row reads:

```text
title
unread
preview
time
important state
```

---

# 136. Focus Preservation

When rows reorder because new message arrives:

```text
keyboard focus stays on same ConversationId
```

not same numeric row index.

---

# 137. Scroll Preservation

If user is far down list and new conversation arrives at top:

```text
do not jump scroll position
```

---

# 138. New Activity Indicator

Optional:

```text
new conversations above
```

if user scrolled far.

---

# 139. Empty Inbox

Display:

```text
No conversations yet
```

Actions:

```text
Start conversation
Add contact
Scan QR
```

---

# 140. Empty Requests

```text
No message requests
```

No extra clutter.

---

# 141. Empty Archive

```text
No archived conversations
```

---

# 142. Loading

Local-first list should usually appear immediately.

Skeleton only on:

```text
first DB load
core startup
large migration
```

---

# 143. Search Rebuilding

Inbox filtering should still work from conversation projection.

No dependency on Part 32 full-text index.

---

# 144. Core Unavailable

Show:

```text
Could not load conversations
Retry
```

with diagnostics if serious.

---

# 145. Storage Full

Inbox remains readable.

Global action banner:

```text
Storage is full. New messages may not be saved.
Manage Storage
```

---

# 146. Security Incident

If account identity/security requires attention:

```text
security banner
```

above list or global shell.

Do not bury in one conversation if account-wide.

---

# 147. Inbox Filters

Recommended initial filters:

```text
All
Unread
Groups
```

Requests and Archived as separate actions.

---

# 148. Filter Persistence

Can persist per device.

---

# 149. Desktop Filter Bar

Compact:

```text
search field
filter icon
```

---

# 150. Android Filter Chips

Optional:

```text
All
Unread
Groups
```

only if frequent use.

---

# 151. Conversation Count

Do not show total count unless useful.

---

# 152. Unread Count at App Level

Chats tab badge derives from aggregate durable unread state.

---

# 153. Requests Count at App Level

Could show separately in Chats badge/menu.

---

# 154. Active Call Indicator

If active call belongs to conversation:

```text
persistent call bar
```

already handles it.

Inbox row can optionally show small call indicator.

---

# 155. Transfer Indicator

If active transfer:

```text
small progress glyph
```

optional.

Avoid showing percentage in every inbox row.

---

# 156. Draft Preview Length

Truncate to one line or two lines.

---

# 157. Message Preview Length

Same.

Avoid expensive rich text in sidebar row.

---

# 158. Preview Sanitization

Do not render:

```text
full Markdown
HTML
```

in conversation row.

Use plain semantic summary.

---

# 159. Emoji

Display naturally.

---

# 160. RTL

Inbox layout should adapt:

```text
avatar
text
timestamp
badges
```

to RTL.

---

# 161. Localization

All semantic preview labels:

```text
Photo
Missed call
Draft
Failed to send
```

localized in UI.

Rust should provide type + safe fields, not English.

---

# 162. Relative Time Updates

Do not tick every second.

Update:

```text
minute/hour/day boundaries
```

as needed.

---

# 163. Timer Efficiency

One shared UI timer can refresh visible timestamp labels.

Do not create timer per row.

---

# 164. Avatar Loading

Use thumbnail/cache.

Show stable placeholder immediately.

---

# 165. Presence Avatar Badge

Ensure accessibility label.

---

# 166. Broken Avatar

Fallback without error icon clutter.

---

# 167. Conversation List Performance

Target smooth scrolling with:

```text
thousands of conversations
```

through virtualization/paging.

---

# 168. Row Render Cost

Keep low:

```text
one avatar
few text nodes
few icons
```

---

# 169. Avoid Per-Row Heavy Subscriptions

Do not subscribe separately to network/presence actors for every row.

Rust/presentation layer should provide aggregated updates.

---

# 170. Visible-Presence Subscription

Part 30 can subscribe presence only for:

```text
visible rows
active conversation
```

to reduce background work.

---

# 171. Visible Row Reporting

UI may report current visible ConversationIds to presentation layer.

---

# 172. Privacy

Do not upload inbox ordering/search/filter behavior as telemetry by default.

---

# 173. Analytics

If enabled, only aggregate metrics:

```text
inbox load time
row count
filter usage
```

without names/previews.

---

# 174. Screenshot Testing

Capture:

```text
empty
normal
many unread
typing
draft
failed message
request
offline
large font
RTL
dark mode
```

---

# 175. Desktop Interaction Tests

Verify:

```text
arrow navigation
Enter open
context menu
archive undo
pin
search filter
```

---

# 176. Android Interaction Tests

Verify:

```text
tap
long press
swipe
filter
FAB/new message
TalkBack
```

---

# 177. Multi-Device Tests

Scenario:

```text
phone reads
desktop unread clears
```

Scenario:

```text
desktop archives
phone row disappears
```

if archive sync enabled.

---

# 178. Typing Test

Typing appears, expires automatically, and does not reorder row.

---

# 179. Presence Test

Presence changes without reordering.

---

# 180. Draft Test

Draft preview supersedes old message preview according to rules.

---

# 181. Failed Send Test

Failure indicator survives restart because outbox state is durable.

---

# 182. Offline Test

Queued outgoing preview remains.

---

# 183. Request Flood Test

Hundreds of unknown requests remain bounded and grouped.

---

# 184. Group Rename Test

Row updates without losing selection/focus.

---

# 185. Delete Open Conversation Test

Row disappears; detail navigates safely.

---

# 186. Pagination Test

Loading older list pages does not duplicate rows.

---

# 187. Stable Ordering Test

Presence/typing/read receipt changes do not reorder unless durable activity changed.

---

# 188. Rust Presentation API

```rust
pub trait InboxPresentation {
    async fn snapshot(
        &self,
        filter: InboxFilter,
    ) -> Result<InboxSnapshot, UiError>;

    async fn next_page(
        &self,
        cursor: ConversationCursor,
    ) -> Result<ConversationPage, UiError>;

    async fn archive(
        &self,
        conversation: ConversationId,
    ) -> Result<(), UiError>;

    async fn set_muted(
        &self,
        conversation: ConversationId,
        muted: bool,
    ) -> Result<(), UiError>;

    async fn set_pinned(
        &self,
        conversation: ConversationId,
        pinned: bool,
    ) -> Result<(), UiError>;
}
```

---

# 189. Inbox Filter

```rust
pub enum InboxFilter {
    All,
    Unread,
    Groups,
    Archived,
    Requests,
}
```

---

# 190. Row UI Model

Platform-specific UI models may mirror the shared DTO.

Android:

```kotlin
data class ConversationRowUiModel(...)
```

Desktop:

```rust
pub struct ConversationRowView(...)
```

---

# 191. Mapping Rule

Platform mapper can change:

```text
formatted time
localized labels
icon resource
```

but not semantic state.

---

# 192. Android ViewModel

Owns:

```text
current filter
search text
scroll restoration metadata
UiEffect
```

Rust owns list truth.

---

# 193. Desktop Presenter

Owns:

```text
selected conversation
filter text
sidebar presentation
keyboard focus identity
```

Rust owns inbox truth.

---

# 194. Selection vs Conversation State

Selected row is UI state.

Unread status is domain state.

---

# 195. Navigation

Click/tap:

```text
ConversationId
→ typed navigation
```

Desktop updates detail pane.

Android pushes conversation destination on compact width.

---

# 196. Adaptive Navigation

On tablet:

```text
row selection
→ detail pane
```

without full back-stack replacement.

---

# 197. Back from Conversation

Phone:

```text
back → inbox
```

Desktop:

```text
sidebar always remains
```

---

# 198. Notification Entry

If notification opens conversation:

```text
inbox may update selection
```

and read state follows actual viewport semantics.

---

# 199. Mark Read from Notification

Inbox unread updates from Rust event.

---

# 200. Inbox Initialization

Startup:

```text
load local snapshot
render
subscribe
attach network/presence later
```

---

# 201. Network Independence

The list must not wait for:

```text
relay
presence
push
Internet
```

to render.

---

# 202. Background Update

Part 31 may receive messages while no UI exists.

When app opens:

```text
snapshot already reflects them
```

---

# 203. Crash Recovery

Inbox itself requires no special event replay in UI.

Request fresh snapshot.

---

# 204. Search Filter after Restart

May restore per-platform preference.

---

# 205. Security Boundary

Conversation summary must not expose:

```text
raw encryption keys
transport addresses
device secrets
```

---

# 206. Unknown Peer Metadata

Avoid rich metadata fetch before acceptance.

---

# 207. Row Action Authorization

Rust validates:

```text
can delete
can leave
can block
```

UI only hides/disables obvious unavailable actions.

---

# 208. Context Capability DTO

Could include:

```rust
pub struct ConversationCapabilities {
    pub can_edit_title: bool,
    pub can_leave: bool,
    pub can_delete_local: bool,
    pub can_block: bool,
}
```

---

# 209. Conversation Row Does Not Need Every Capability

Fetch detailed capabilities when action menu opens if expensive.

---

# 210. Undo Architecture

Archive/mute actions can return:

```text
UndoToken
```

if truly supported.

---

# 211. UI/UX Part 04 Definition of Done

Part 04 is complete when:

- conversation list is a projection of Rust durable state
- pinned/recent/requests/archived semantics are defined
- typing/presence enrich rows without reordering them
- unread counts derive from durable read cursors
- mark unread is distinguished from remote read receipt reversal
- draft, failed-send, queued-send, file, call, and normal message previews are defined
- unknown message requests remain separated from trusted inbox
- request acceptance does not imply identity verification
- desktop Dioxus uses a persistent sidebar with keyboard/context-menu actions
- Android Compose uses a native touch list with long-press/swipe alternatives
- tablet/foldable list-detail layouts are defined
- row actions share Rust commands
- archive/mute/pin state can sync according to explicit policy
- message list ordering depends on durable activity, not presence/typing
- pagination/virtualization and stable `ConversationId` keys are mandatory
- list remains useful offline
- background-delivered messages appear from fresh snapshot without UI-specific reconciliation
- accessibility, RTL, large font, focus preservation, and screen-reader row semantics are defined
- failure, request flood, deletion, group rename, multi-device, presence, typing, and pagination tests are included

---

# 212. Final Cross-Platform Architecture

```text
                 RUST INBOX PROJECTION
                        │
      ┌─────────────────┼─────────────────┐
      │                 │                 │
 Durable State      Ephemeral         Preferences
 messages/read      typing            pin/mute/archive
 outbox/activity    presence
      │                 │                 │
      └─────────────────┼─────────────────┘
                        │
                ConversationSummary
                 ┌──────┴──────┐
                 │             │
              Dioxus        Compose
                 │             │
          Sidebar/List     LazyColumn
                 │             │
                 └──────┬──────┘
                        │
                ConversationId
                        │
                 Typed Navigation
```

---

# 213. Final Principle

The inbox should feel:

```text
immediate
stable
predictable
private
offline-capable
```

It should not feel like a noisy live dashboard where every typing or presence update reshuffles the UI.

The right model is:

```text
durable conversation order
+
clear unread/read state
+
temporary presence/typing overlays
+
platform-native interactions
+
shared Rust truth
```

This makes the conversation list a reliable entry point for the entire communication product across both desktop and Android.
