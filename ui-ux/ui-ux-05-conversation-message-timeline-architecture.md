# UI/UX Part 05 — Conversation / Message Timeline UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 05  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete conversation screen and message timeline experience across desktop and Android, including paging, scroll anchoring, unread boundaries, message grouping, replies, edits, deletes, reactions, attachments, voice notes, delivery/read state, failed/pending messages, typing overlays, search navigation, accessibility, and large-history performance.

---

# 1. Purpose

The conversation timeline is the heart of the communication product.

It must remain understandable and stable while handling:

```text
live incoming messages
offline queues
edited messages
deleted messages
replies
attachments
voice notes
read receipts
typing
message search
history pagination
multi-device sync
reconnects
```

The governing principle is:

> **The timeline is a stable chronological projection of durable message truth, with ephemeral overlays layered on top without destabilizing reading position.**

---

# 2. Architectural Position

```text
Rust Conversation Projection
        │
        ├── paged messages
        ├── edits/deletes
        ├── reactions
        ├── delivery/read state
        ├── attachment metadata
        ├── reply references
        └── outbox state
        │
        + Ephemeral overlay
            ├── typing
            ├── recording
            └── presence
        │
        ▼
ConversationScreenSnapshot
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Conversation Screen Regions

Shared semantic regions:

```text
Header
Message Timeline
Unread / Date Separators
Typing / Recording Indicator
Reply/Edit Context
Composer
Attachment / Transfer Context
```

Desktop may add a persistent inspector.

Android uses a focused single-screen composition.

---

# 4. Conversation Header

Shows:

```text
title
avatar
presence/availability
verification/security state if relevant
audio call action
video call action
search
details
```

Do not overload with backend diagnostics.

---

# 5. Header Presence

Examples:

```text
Online
Away
Last seen recently
```

according to privacy settings.

If unknown:

```text
show nothing
```

rather than false offline precision.

---

# 6. Header Security State

Show only meaningful actionable state:

```text
Verified
Identity changed
Unverified device warning
```

No decorative lock clutter.

---

# 7. Timeline Data Model

```rust
pub struct ConversationScreenSnapshot {
    pub conversation: ConversationHeaderView,
    pub messages: Vec<MessageView>,
    pub oldest_cursor: Option<MessageCursor>,
    pub newest_cursor: Option<MessageCursor>,
    pub unread_boundary: Option<MessageSequence>,
    pub typing: TypingSummary,
    pub recording: RecordingSummary,
}
```

---

# 8. Message Identity

Every row keyed by:

```text
MessageId
```

Never list index.

---

# 9. Message Sequence

Conversation ordering uses stable logical sequence:

```rust
pub struct MessageSequence(pub u64);
```

Do not use wall-clock timestamps alone for ordering.

---

# 10. Message View

```rust
pub struct MessageView {
    pub id: MessageId,
    pub sequence: MessageSequence,
    pub sender: SenderView,
    pub direction: MessageDirection,
    pub content: MessageContentView,
    pub sent_at: Timestamp,
    pub edited: bool,
    pub deleted: bool,
    pub delivery: MessageDeliveryView,
    pub reply_to: Option<MessageReferenceView>,
    pub reactions: Vec<ReactionSummary>,
}
```

---

# 11. Message Direction

```rust
pub enum MessageDirection {
    Incoming,
    Outgoing,
    System,
}
```

---

# 12. Initial Load

Open conversation:

```text
load recent window around newest message
```

Example:

```text
50–100 messages
```

Tune by device/performance.

---

# 13. Local-First Open

Existing conversation should open from local state immediately.

Do not wait for:

```text
presence
relay
sync
Internet
```

---

# 14. History Pagination

Scrolling upward:

```text
request older page
prepend
preserve anchor
```

---

# 15. Scroll Anchor

Use:

```text
MessageId + visual offset
```

not numeric index.

---

# 16. Anchor Preservation

When older rows prepend:

```text
same visible message stays at same screen location
```

---

# 17. New Messages While at Bottom

If user is at bottom:

```text
append
auto-follow
```

---

# 18. New Messages While Reading History

If user is not near bottom:

```text
append invisibly
show "N new messages" chip
```

Do not force scroll.

---

# 19. New Message Chip

Action:

```text
jump to newest
```

May display:

```text
3 new messages
```

---

# 20. Unread Boundary

When opening unread conversation:

```text
show separator
"New messages"
```

before first unread message.

---

# 21. Unread Boundary Persistence

Boundary derives from durable read cursor.

UI does not invent it.

---

# 22. Marking Read

Part 30 semantics.

UI reports highest truly read sequence based on:

```text
visibility
focus/lifecycle
```

---

# 23. Desktop Read Criteria

Recommended:

```text
conversation active
window focused
message sufficiently visible
```

---

# 24. Android Read Criteria

Recommended:

```text
Activity RESUMED
conversation destination visible
message sufficiently visible
device not obscuring app
```

---

# 25. Do Not Mark Entire Conversation Read on Open

If user opens and immediately leaves:

```text
only truly viewed range
```

should advance read cursor.

---

# 26. Date Separators

Show when calendar grouping changes:

```text
Today
Yesterday
Monday
12 Aug 2026
```

localized.

---

# 27. Date Separator Computation

Can be presentation-layer derived from timestamps.

---

# 28. Message Grouping

Consecutive messages from same sender within a short interval can visually group.

---

# 29. Grouping Criteria

Example:

```text
same sender
same message type class
time gap < 5 minutes
no major system boundary
```

---

# 30. Grouped Rendering

First message may show:

```text
avatar
sender name
timestamp
```

following messages reduce repeated chrome.

---

# 31. Do Not Group Across

```text
date separator
unread boundary
system event
security event
large time gap
```

---

# 32. Group Message Sender Name

Direct conversation:

```text
usually unnecessary
```

Group conversation:

```text
show sender name
```

---

# 33. Message Bubble vs Flat Layout

Android may use bubble-oriented layout.

Desktop can use:

```text
bubbles
or
clean aligned timeline
```

but semantics remain identical.

---

# 34. Outgoing Alignment

Typically:

```text
right
```

in LTR.

RTL should adapt appropriately without confusing sender identity.

---

# 35. RTL

Use layout direction correctly.

Do not hardcode outgoing=invariant physical right if product decides semantic alignment differently in RTL.

---

# 36. Message Width

Limit maximum readable width.

Do not stretch one-line message across ultrawide desktop.

---

# 37. Text Rendering

Support:

```text
Unicode
emoji
RTL
mixed scripts
links
basic formatting
```

---

# 38. Rich Text

If Markdown-like formatting supported:

```text
bold
italic
code
quote
links
```

must be sanitized.

---

# 39. No Arbitrary HTML

Do not render untrusted HTML from message content.

---

# 40. Links

Detect and render clickable links.

---

# 41. External Link Safety

Open via platform browser after scheme validation.

Warn on suspicious/unsupported schemes.

---

# 42. Link Preview

Optional enriched message content.

Preview metadata is derived.

If unavailable:

```text
message remains valid
```

---

# 43. Link Preview Privacy

Avoid fetching preview server-side in a way that leaks user IP/contact graph unintentionally.

Use privacy-aware backend policy.

---

# 44. Replies

Reply creates reference:

```text
MessageId
```

not copied-only text.

---

# 45. Reply Preview

Message bubble shows:

```text
sender
small quoted excerpt/type
```

---

# 46. Reply Navigation

Tap/click quoted reply:

```text
jump to referenced MessageId
```

---

# 47. Missing Reply Target

If original unavailable/deleted:

```text
Original message unavailable
```

not broken layout.

---

# 48. Reply Highlight

After jump:

```text
brief highlight
```

respect reduced motion.

---

# 49. Editing

Editable only if Rust capability says allowed.

---

# 50. Edit UI

Entering edit mode:

```text
composer populated
edit context banner
```

---

# 51. Save Edit

Rust performs authoritative validation.

---

# 52. Edited Label

Show compact:

```text
edited
```

---

# 53. Edit History

Optional future:

```text
view edit history
```

not required initially.

---

# 54. Delete

Possible states:

```text
Delete for me
Delete for everyone
```

depending protocol/capability.

---

# 55. Deleted Message Placeholder

If tombstone retained:

```text
Message deleted
```

---

# 56. Local-Only Delete

May remove row entirely or show local tombstone according to history model.

---

# 57. Reactions

Reaction bar below/near message.

Examples:

```text
👍 3
❤️ 2
```

---

# 58. Reaction Interaction

Desktop:

```text
hover action
context menu
click existing reaction
```

Android:

```text
long press
reaction picker
tap existing reaction
```

---

# 59. Reaction Summary

Rust provides aggregated:

```rust
pub struct ReactionSummary {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
}
```

---

# 60. Reaction Details

Optional:

```text
who reacted
```

sheet/dialog.

---

# 61. Reaction Accessibility

Screen reader:

```text
3 thumbs up reactions
```

---

# 62. Attachments

Message content can include:

```text
image
video
document
audio
generic file
```

---

# 63. Attachment State

```rust
pub enum AttachmentState {
    RemoteAvailable,
    Queued,
    Downloading,
    Verifying,
    Ready,
    Failed,
    Removed,
}
```

---

# 64. Attachment Card

Shows:

```text
name/type
size
thumbnail if available
transfer state
download/open action
```

---

# 65. Image Message

Use:

```text
thumbnail
aspect-ratio-preserving container
```

---

# 66. Progressive Image

```text
placeholder
thumbnail
full image on open
```

---

# 67. Video Message

Show:

```text
thumbnail
duration
play
download state
```

---

# 68. Document Message

Show:

```text
filename
type
size
download/open
```

---

# 69. File Transfer Progress

Do not update message row at packet frequency.

Throttle/coalesce.

---

# 70. Attachment Failure

Show:

```text
Retry
```

if recoverable.

---

# 71. Attachment Missing

Example:

```text
File no longer available
```

---

# 72. Voice Notes

Message card shows:

```text
play/pause
progress
duration
speed
waveform optional
download state
```

---

# 73. Voice Note Waveform

Derived visualization.

Do not make waveform a requirement if unavailable.

---

# 74. Playback Speed

Examples:

```text
1×
1.5×
2×
```

---

# 75. Voice Note Scrubbing

Support seek if media format/index allows.

---

# 76. Voice Note Read State

Playing audio is separate from message read state.

---

# 77. Media Viewer

Tap image/video/file:

```text
open dedicated viewer
```

Desktop may use detached window.

Android may use full-screen destination.

---

# 78. Media Viewer Navigation

Allow:

```text
previous/next media in conversation
```

using logical media query.

---

# 79. Pending Outgoing Message

After send command:

```text
Rust creates durable local pending message
```

Timeline renders immediately.

---

# 80. Pending Visual

Small:

```text
clock
Queued
Sending
```

depending state.

---

# 81. Offline Pending

User sees message in place.

No modal network error.

---

# 82. Sent

Local outbox has completed sender semantics.

---

# 83. Delivered

Recipient application durably accepted.

---

# 84. Read

Recipient read cursor advanced through message.

---

# 85. Failure

Permanent/retry-needed failure:

```text
warning icon
Tap/click to retry
```

---

# 86. Retry

Uses same logical MessageId/idempotency where backend requires.

Do not duplicate visible message.

---

# 87. Retry UX

Action:

```text
Retry
```

Optional:

```text
Delete
Details
```

---

# 88. Message Info

Shows:

```text
sent time
delivered time
read time
delivery devices only if product exposes
```

Privacy rules apply.

---

# 89. Group Delivery Info

Potential:

```text
Delivered to 5
Read by 3
```

---

# 90. Group Detail View

Optional per-member receipt list for small groups.

---

# 91. Typing Indicator

Appears:

```text
above composer
or
bottom of timeline
```

---

# 92. Recommendation

Place typing indicator just above composer / at bottom of timeline so it feels associated with incoming message flow.

---

# 93. Typing Should Not Occupy Message Row Identity

It is not a durable message.

---

# 94. Recording Indicator

Similar:

```text
Alice is recording audio…
```

---

# 95. Typing While Scrolled Up

Do not force scroll.

Indicator can stay near composer.

---

# 96. Presence

Header-level status is usually sufficient.

Avoid presence labels on every message.

---

# 97. System Messages

Examples:

```text
Alice joined
Device verified
Group renamed
Call ended
```

Use centered/system style.

---

# 98. Security Events in Timeline

Only if conversation-relevant.

High-risk event may include:

```text
Identity changed
```

with action:

```text
Review
```

---

# 99. Do Not Fake Security Events

Security events originate from Rust/security subsystem.

---

# 100. Calls in Timeline

Call history event:

```text
Audio call — 12 min
Missed call
Video call
```

---

# 101. Call Event Actions

```text
Call back
```

---

# 102. File Events

Sending file is usually represented as message attachment, not separate duplicate system event.

---

# 103. Selection Mode

Desktop:

```text
mouse/keyboard multi-select
```

Android:

```text
long press → contextual selection mode
```

---

# 104. Multi-Select Actions

Potential:

```text
Copy
Forward
Delete
Save attachments
```

depending capabilities.

---

# 105. Selection Stability

Key by MessageId.

Reordering/edit must not lose selection accidentally.

---

# 106. Desktop Selection

Shift-click range.

Ctrl/Cmd-click toggle.

---

# 107. Android Selection

Long-press starts selection.

Tap toggles.

Back exits selection before leaving conversation.

---

# 108. Context Actions

Message actions must be capability-driven.

---

# 109. Capability Model

```rust
pub struct MessageCapabilities {
    pub can_reply: bool,
    pub can_edit: bool,
    pub can_delete_local: bool,
    pub can_delete_everywhere: bool,
    pub can_forward: bool,
    pub can_react: bool,
    pub can_copy: bool,
}
```

---

# 110. Context Menu / Bottom Sheet

Desktop:

```text
right click
hover menu
keyboard
```

Android:

```text
long press
bottom sheet/contextual toolbar
```

---

# 111. Copy

Copies message text only if content type allows.

---

# 112. Forward

Select destination conversation(s).

Rust creates proper forwarded message representation.

---

# 113. Forward Privacy

Do not automatically expose original sender identity if product semantics do not require it.

---

# 114. Search Within Conversation

Desktop:

```text
Ctrl/Cmd+F
```

Android:

```text
search action in app bar
```

---

# 115. Search Results

Part 32 returns MessageIds.

Timeline supports:

```text
jump to result
highlight
next/previous result
```

---

# 116. Search Navigation State

Do not duplicate search engine.

UI maintains current result index only.

---

# 117. Jump Loading

If MessageId is far in history:

```text
request page centered around target
```

---

# 118. Centered Page Request

```rust
pub struct MessageAroundRequest {
    pub conversation: ConversationId,
    pub anchor: MessageId,
    pub before: usize,
    pub after: usize,
}
```

---

# 119. Jump Highlight

Temporary visual emphasis.

---

# 120. Return to Latest

After search/jump:

```text
button to return to newest
```

if far from bottom.

---

# 121. Scroll-To-Bottom Button

Appears when:

```text
user far from newest
```

May include unread/new count.

---

# 122. Desktop Timeline Layout

Recommended:

```text
Header
────────────────────────
Virtualized Message List
────────────────────────
Typing / Reply Context
Composer
```

Optional right inspector.

---

# 123. Android Timeline Layout

Recommended:

```text
TopAppBar
LazyColumn timeline
Typing indicator
Reply/Edit context
Composer with IME insets
```

---

# 124. Reverse List vs Normal List

Compose implementations often use reverse layout.

Architecture should choose based on:

```text
paging
anchor stability
accessibility
```

not habit.

---

# 125. Recommendation

Use a model that keeps chronological semantics clear and supports robust prepend paging.

Implementation can use reverse layout internally if thoroughly tested.

---

# 126. Desktop Virtualization

Dioxus timeline must virtualize long histories.

---

# 127. Variable Height Rows

Messages have variable height.

Virtualizer must handle:

```text
text
images
replies
files
reactions
```

---

# 128. Estimated Heights

Can use estimated heights but must correct without large jumps.

---

# 129. Image Dimension Metadata

Know image aspect ratio before full image decode if possible to reserve layout space.

---

# 130. Avoid Layout Shift

Reserve attachment bounds.

---

# 131. Android LazyColumn

Use:

```text
key = MessageId
contentType = message type
```

where helpful.

---

# 132. Compose Recomposition Granularity

Delivery tick/reaction change should recompose affected row, not whole conversation.

---

# 133. Desktop Signal Granularity

Same.

---

# 134. Message Row Presentation Model

Can be specialized:

```rust
pub enum MessageContentView {
    Text(TextMessageView),
    Image(ImageMessageView),
    Video(VideoMessageView),
    File(FileMessageView),
    Voice(VoiceMessageView),
    System(SystemMessageView),
}
```

---

# 135. Platform Mapping

Android maps to sealed UI model.

Desktop uses Rust enum directly/presenter mapping.

---

# 136. Time Display

Do not show timestamp on every grouped message if visually noisy.

Possible:

```text
show on group end
show on hover desktop
show small always mobile
```

---

# 137. Accessibility Timestamp

Even if hidden visually, timestamp should remain available to screen reader/details.

---

# 138. Desktop Hover Details

Hover can reveal:

```text
timestamp
actions
```

but nothing essential should exist only on hover.

---

# 139. Android Touch Interaction

Tap on message should not always open action sheet.

Recommended:

```text
tap content-specific
long press actions
```

---

# 140. Double Tap

Optional quick reaction.

Do not make critical action rely on it.

---

# 141. Message Spacing

Use smaller spacing inside grouped runs.

Larger spacing between sender/time groups.

---

# 142. System Event Spacing

Distinct but not oversized.

---

# 143. Conversation Background

Neutral.

Do not reduce text contrast with decorative wallpapers by default.

---

# 144. Custom Wallpaper

Optional later.

Must preserve readability/accessibility.

---

# 145. Theme Contrast

Incoming/outgoing bubbles need sufficient contrast in light/dark modes.

---

# 146. Color Is Not Sole Sender Indicator

Use alignment/shape/name as well.

---

# 147. Large Font

Timeline remains usable at large font scale.

Message action buttons must not overlap.

---

# 148. Android IME Insets

Composer stays above keyboard.

Timeline adjusts without losing anchor.

---

# 149. Keyboard Open/Close

Do not unexpectedly jump to bottom if user was reading history.

---

# 150. Composer Expansion

Multiline text expands up to limit.

Then scroll inside composer.

---

# 151. Reply Context

Above composer:

```text
Replying to Alice
excerpt
X cancel
```

---

# 152. Edit Context

```text
Editing message
```

distinct from reply.

---

# 153. Attachment Draft Strip

Selected attachments preview above composer.

Detailed composer architecture belongs Part 06.

---

# 154. Timeline During Attachment Upload

Pending attachment message remains in timeline.

---

# 155. Message Send Ordering

If user sends rapidly:

```text
local sequence/order is stable
```

even if network completions differ.

---

# 156. Server/Peer Reordering

Rust resolves canonical logical order.

UI should not sort by arrival time.

---

# 157. Timestamp Corrections

If message timestamp adjusted after sync:

```text
do not cause arbitrary reorder if logical sequence stable
```

---

# 158. Duplicate Delivery

Same MessageId arriving via direct/relay/DTN:

```text
one row
```

---

# 159. Edit Race

If user opens action menu while message edited remotely:

```text
capability/state refresh
```

before commit.

---

# 160. Delete Race

If message disappears while selected:

```text
selection removes safely
```

---

# 161. Reaction Race

Use optimistic display only if Rust returns accepted pending state.

---

# 162. Multi-Device Sent Message

Message sent from user's other device should appear as outgoing.

---

# 163. Sender Device Detail

Do not clutter timeline with device identity.

Message info may show:

```text
Sent from Desktop
```

only for own-device diagnostics if useful.

---

# 164. Read Cursor Sync

Other device reading conversation updates local unread boundary/badge.

Do not forcibly move scroll.

---

# 165. Message Expiration

If disappearing-message feature later exists:

```text
expired row removed/tombstoned
```

without destabilizing scroll excessively.

---

# 166. Expiry Countdown

Avoid second-by-second countdown on every message.

Show coarse info/details.

---

# 167. Pinned Messages

Future conversation feature.

Could show:

```text
Pinned message banner
```

in header.

Not required for basic v1.

---

# 168. Bookmarks / Saved Messages

Could be local metadata.

Not necessary in first timeline version.

---

# 169. Threads

If future threads:

```text
reply count
open thread
```

Need separate UI architecture later.

Do not overbuild now.

---

# 170. Message Translation

Future optional feature.

Do not couple base row to cloud translation.

---

# 171. AI Actions

Future optional:

```text
summarize
rewrite
```

must respect privacy and explicit user action.

Not part of core timeline.

---

# 172. Unknown Message Type

Forward compatibility.

Render:

```text
Unsupported message type
Update app to view
```

---

# 173. Unknown Optional Fields

Ignore safely.

---

# 174. Unsupported Attachment

Show metadata and:

```text
Open externally
```

if safe.

---

# 175. Corrupt Message

Do not crash.

Show:

```text
Could not display this message
```

with optional diagnostics.

---

# 176. Decryption Failure

Security-sensitive.

Show generic failure and route to diagnostics/security if needed.

Do not expose raw crypto details.

---

# 177. Blocked Sender Historical Messages

Existing history may remain visible.

Future incoming content blocked.

---

# 178. Request Conversation Timeline

Before acceptance:

```text
restricted actions
no read receipt
no typing share
no auto-download
```

according to Part 28/30.

---

# 179. Message Request Banner

Show:

```text
This person is not in your contacts
Accept
Block
Delete
```

above timeline.

---

# 180. Group Timeline

Show sender names/avatars more clearly.

---

# 181. Group Membership Events

Centered system rows:

```text
Alice joined
Bob left
Carol changed group name
```

---

# 182. Group Security Change

If meaningful:

```text
Security settings changed
```

with details action.

---

# 183. Calls in Group Timeline

Group call started/ended event if product wants durable history.

---

# 184. Delivery State in Group

Do not show dozens of receipt icons inline.

Use compact aggregate.

---

# 185. Message Details Group

Dedicated sheet/dialog for:

```text
delivered/read participants
```

---

# 186. Accessibility — Message Row

Screen reader should expose coherent information:

```text
Alice, 10:42 AM, "See you tomorrow", 2 reactions
```

---

# 187. Incoming/Outgoing Accessibility

Include:

```text
You:
```

for outgoing if needed.

---

# 188. Delivery Accessibility

Announce:

```text
Delivered
Read
Failed
```

for outgoing.

---

# 189. Attachment Accessibility

Example:

```text
PDF file, timetable.pdf, 2.4 megabytes, downloaded
```

---

# 190. Voice Note Accessibility

Expose:

```text
Voice message, 34 seconds, play
```

---

# 191. Reply Accessibility

Announce:

```text
Replying to Alice: ...
```

---

# 192. Reaction Accessibility

Expose summarized counts and action.

---

# 193. Focus After Sending

Composer stays focused by default on desktop.

Android may keep keyboard open if user continues conversation.

---

# 194. Focus After Reply Navigation

Do not unexpectedly focus composer.

---

# 195. Focus After Search Jump

Focus timeline/result target appropriately.

---

# 196. New Message Accessibility

Use a controlled live region.

Do not read every rapidly arriving message automatically in large group unless user preference allows.

---

# 197. Selection Accessibility

Announce:

```text
1 message selected
2 messages selected
```

---

# 198. Performance — Large History

Test:

```text
10k
100k
1M
```

message histories where architecture scale requires.

Only visible window rendered.

---

# 199. Memory

Memory use should not grow linearly with total history.

---

# 200. Paging Cache

Maintain bounded nearby pages.

---

# 201. Page Eviction

Far-old loaded pages can be released while preserving anchor metadata.

---

# 202. Search Jump Cache

Keep temporary around-target window.

---

# 203. Attachment Thumbnail Cache

Bound separately.

---

# 204. Message Row Cache

Avoid caching giant rich render trees indefinitely.

---

# 205. High-Frequency Updates

Typing:

```text
few Hz maximum
```

Transfer:

```text
5–10 Hz
```

Audio level not part of normal message row.

---

# 206. Timestamp Refresh

Shared timer.

No per-message timer.

---

# 207. Scroll Performance

Heavy operations must not run on UI thread:

```text
decryption
file hashing
thumbnail generation
search
```

---

# 208. Dioxus Desktop Interaction

Mouse:

```text
hover actions
right click
select text
drag attachment
```

Keyboard:

```text
Up/Down history navigation only when appropriate
Ctrl/Cmd+F search
Esc cancel reply/edit/selection
```

---

# 209. Android Interaction

Touch:

```text
tap
long press
swipe where appropriate
```

Use native haptics sparingly.

---

# 210. Desktop Text Selection

Users should be able to select/copy message text naturally.

---

# 211. Android Text Selection

Long-press text can support copy if not conflicting with message action gesture.

May expose explicit Copy action instead for consistency.

---

# 212. Copy Formatting

Default copy:

```text
plain text
```

Optional:

```text
Copy with formatting
```

later.

---

# 213. Code Blocks

If supported:

```text
monospace
horizontal scrolling
copy button
```

---

# 214. Long Messages

Collapse only if extremely long and UX benefits.

Default should show full content.

---

# 215. Spoilers

Optional future content type.

---

# 216. Sensitive Media Blur

Optional:

```text
tap to reveal
```

per conversation/security setting.

---

# 217. Timeline Empty State

New conversation:

```text
No messages yet
Say hello
```

---

# 218. Request Empty State

If no accepted messages but request context exists:

```text
Message request
```

banner remains.

---

# 219. Core Loading

If recent page unavailable:

```text
small loading skeleton
```

---

# 220. Older History Loading

Top loader/spinner.

Do not block current messages.

---

# 221. History End

At oldest:

```text
no endless spinner
```

Optional subtle:

```text
Beginning of conversation
```

---

# 222. History Gap

If local data missing:

```text
Load older messages
```

or:

```text
History unavailable on this device
```

depending sync model.

---

# 223. Archive Node History

If trusted archive can supply old messages:

```text
Load from archive
```

future feature.

---

# 224. Message Integrity Problem

If message fails verification:

```text
This message could not be verified
```

security handling from Part 28.

---

# 225. Screenshots / Testing States

Required screenshot fixtures:

```text
empty
normal direct
group
unread boundary
reply
edited
deleted
reactions
image
file
voice note
failed outgoing
queued offline
typing
request
dark mode
RTL
large font
```

---

# 226. Timeline Interaction Tests

Verify:

```text
prepend paging
anchor preservation
new message while scrolled up
jump to reply
search jump
selection
edit
delete
retry
```

---

# 227. Android Tests

Include:

```text
IME open/close
rotation
process recreation
TalkBack
large font
long press selection
```

---

# 228. Desktop Tests

Include:

```text
right click
keyboard search
text selection
multi-select
drag/drop
```

---

# 229. Multi-Device Tests

Scenario:

```text
message sent on desktop
appears outgoing on phone
```

Scenario:

```text
phone reads
desktop delivery/read state updates
```

---

# 230. Offline Tests

Send several messages offline.

Expected:

```text
stable local order
queued state
no duplicate on reconnect
```

---

# 231. Duplicate Delivery Test

Same MessageId through multiple routes:

```text
one row
```

---

# 232. Edit/Delete Race Tests

Remote edits/deletes while user viewing/selected.

No crash or stale action.

---

# 233. Pagination Race

Older page arrives while new messages append.

Anchor/order remain correct.

---

# 234. Search Jump Race

Target edited/deleted during navigation.

Fail gracefully.

---

# 235. Attachment Failure Test

Transfer failure updates only affected row.

---

# 236. Group Reaction Flood

Many reaction changes remain bounded/recompose local row only.

---

# 237. Rust Presentation API

```rust
pub trait ConversationPresentation {
    async fn open(
        &self,
        conversation: ConversationId,
    ) -> Result<ConversationScreenSnapshot, UiError>;

    async fn older(
        &self,
        conversation: ConversationId,
        cursor: MessageCursor,
    ) -> Result<MessagePage, UiError>;

    async fn around(
        &self,
        request: MessageAroundRequest,
    ) -> Result<MessagePage, UiError>;

    async fn send(
        &self,
        command: SendMessageCommand,
    ) -> Result<MessageView, UiError>;

    async fn retry(
        &self,
        message: MessageId,
    ) -> Result<(), UiError>;

    async fn edit(
        &self,
        command: EditMessageCommand,
    ) -> Result<MessageView, UiError>;

    async fn delete(
        &self,
        command: DeleteMessageCommand,
    ) -> Result<(), UiError>;
}
```

---

# 238. Timeline Events

```rust
pub enum ConversationUiEvent {
    MessageInserted(MessageView),
    MessageUpdated(MessageView),
    MessageRemoved(MessageId),
    DeliveryChanged {
        message: MessageId,
        state: MessageDeliveryView,
    },
    ReactionsChanged {
        message: MessageId,
        reactions: Vec<ReactionSummary>,
    },
    TypingChanged(TypingSummary),
    RecordingChanged(RecordingSummary),
    HeaderChanged(ConversationHeaderView),
}
```

---

# 239. Event Locality

Only update affected:

```text
message row
header
typing region
```

---

# 240. Android ViewModel

Owns:

```text
reply target
edit target
selection set
search navigation state
scroll-restoration metadata
UiEffects
```

Rust owns message truth.

---

# 241. Dioxus Presenter

Owns:

```text
current selection
reply/edit presentation state
scroll anchor
search navigation presentation
```

---

# 242. Reply/Edit Draft State

Draft text may be UI-local or Rust durable draft service.

Architecture should support later persistence.

---

# 243. Scroll Restoration

Store:

```text
anchor MessageId
offset
```

not row number.

---

# 244. Android Process Death

On recreation:

```text
open conversation snapshot
restore anchor if valid
restore draft if policy supports
```

Do not restore old typing state.

---

# 245. Desktop Window Reopen

Same.

---

# 246. Security Boundary

Timeline never receives:

```text
raw keys
transport secrets
internal ratchet state
```

---

# 247. Unknown Message Extensions

Plugin/extension content can render through safe registered renderer.

---

# 248. Plugin Renderer Boundary

Plugin should not inject arbitrary unsafe UI code into core process.

Use:

```text
declarative safe rendering
or
sandboxed extension surface
```

from Parts 21–24.

---

# 249. Unsupported Plugin Message

Show fallback:

```text
Message requires plugin X
```

with safe install/details action.

---

# 250. Telemetry Privacy

Do not collect:

```text
message content
scroll-reading behavior
reply content
```

by default.

---

# 251. Safe Performance Metrics

```text
timeline render latency
page load latency
recomposition count
scroll frame drops
```

without content.

---

# 252. Definition of Done

UI/UX Part 05 is complete when:

- the conversation timeline is driven by Rust paged message projections
- every message row uses stable `MessageId`
- logical ordering uses stable sequence rather than arrival time
- prepend pagination preserves scroll anchor
- incoming messages do not yank users reading history
- unread boundary comes from durable read cursor
- date separators and message grouping are well-defined
- replies navigate by MessageId and tolerate missing targets
- edit/delete/reaction capabilities come from Rust
- outgoing queued/sent/delivered/read/failed states are clearly represented
- retry does not create duplicate messages
- attachments, images, videos, files, and voice notes have defined states
- media transfer progress is throttled and localized to affected rows
- typing/recording remain ephemeral overlays, not timeline rows
- system/security/call events have distinct semantic rendering
- search can jump to messages outside current loaded page
- desktop Dioxus supports context menus, keyboard, selection, and text copy
- Android Compose supports long-press selection, IME-safe layout, lifecycle recreation, and touch interactions
- large histories are virtualized/paged and memory-bounded
- accessibility, RTL, large-font, screen-reader, and reduced-motion behavior are explicit
- process restart restores durable truth and scroll/draft presentation safely
- duplicate delivery, edit/delete races, pagination races, offline queues, and multi-device updates are covered by tests

---

# 253. Final Architecture

```text
                 RUST CONVERSATION STATE
                          │
               Paged Message Projection
                          │
       ┌──────────────────┼──────────────────┐
       │                  │                  │
   Durable Rows       Delivery State     Attachments
       │                  │                  │
       └──────────────────┼──────────────────┘
                          │
                 Conversation Snapshot
                          │
          ┌───────────────┴───────────────┐
          │                               │
       Dioxus                          Compose
          │                               │
  Virtualized Timeline                LazyColumn
          │                               │
          └──────────────┬────────────────┘
                         │
                 Stable MessageId
                         │
             Reply / Edit / Search
```

Ephemeral overlay:

```text
Typing
Recording
Presence
```

is rendered around the timeline but never becomes durable row truth.

---

# 254. Final Principle

The timeline should protect the user's reading position and mental context even while the distributed system changes underneath it.

The right model is:

```text
stable logical ordering
+
durable message truth
+
anchor-preserving pagination
+
localized incremental updates
+
ephemeral overlays
+
platform-native interaction
```

not:

```text
re-render and re-sort the entire conversation whenever anything changes
```

This gives both Dioxus desktop and Jetpack Compose Android a fast, predictable, accessible conversation experience over the shared Rust communication engine.
