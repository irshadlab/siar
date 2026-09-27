# UI/UX Part 14 — Presence, Typing, Receipts & Status UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 14  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete user-facing presence, typing, recording, delivery/read receipts, availability/status, last-seen, privacy, TTL/staleness, group aggregation, multi-device semantics, offline ambiguity, accessibility, and Rust presentation contracts across desktop and Android.

---

# 1. Purpose

Presence and receipts look simple in the UI, but they are among the easiest features to make misleading.

A user should not infer certainty where the distributed system only has partial information.

Examples:

```text
"offline" may actually mean unknown
"last seen 2 minutes ago" may be stale
"typing" must expire
"read" is usually sequence-based, not per-message polling
one user may have several devices
a muted device may still be online
a group may have too many participants for per-user live state
```

The governing principle is:

> **Ephemeral social state should communicate useful hints without pretending to be stronger evidence than the protocol actually provides.**

---

# 2. Architectural Position

```text
Rust Presence / Ephemeral State
        │
        ├── account presence
        ├── device observations
        ├── availability
        ├── typing
        ├── recording
        ├── read cursors
        └── delivery state
        │
        ▼
Presence Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Presence vs Availability

These are related but distinct.

Presence:

```text
Online
Away
Unknown
```

Availability:

```text
Available
Busy
Do Not Disturb
Invisible
```

Do not collapse both into one dot.

---

# 4. Presence State

Recommended:

```rust
pub enum PresenceState {
    Online,
    Away,
    Unknown,
}
```

Avoid hard `Offline` unless the protocol can prove it.

---

# 5. Why `Unknown` Matters

A device may be:

```text
asleep
out of network
battery constrained
behind delayed transport
not publishing presence
```

So absence of presence is not always proof of offline state.

---

# 6. Last Seen

Optional feature.

```rust
pub struct LastSeenView {
    pub at: Timestamp,
    pub confidence: PresenceConfidence,
}
```

---

# 7. Last Seen Privacy

Users may choose:

```text
Everyone
Contacts
Nobody
```

or equivalent.

---

# 8. Last Seen Precision

Recommended user-facing formatting:

```text
just now
5 minutes ago
today
yesterday
recently
```

depending privacy setting.

Avoid exact minute precision if policy intentionally coarsens.

---

# 9. Presence Confidence

```rust
pub enum PresenceConfidence {
    Live,
    Recent,
    Stale,
}
```

Normal UI rarely shows this directly.

It affects wording.

---

# 10. Presence TTL

Rust owns expiry.

UI does not run its own truth-deciding timer.

---

# 11. Presence Update Flow

```text
network observation
→ Rust presence aggregator
→ TTL/hysteresis
→ PresenceSummary
→ UI
```

---

# 12. Multi-Device Presence

A user may have:

```text
phone online
desktop asleep
tablet offline
```

UI should usually show account-level aggregate.

---

# 13. Account-Level Presence

Possible policy:

```text
Online if any authorized device is live
Away if at least one recent device exists but none active
Unknown otherwise
```

Exact rule belongs in Rust.

---

# 14. Do Not Show Per-Device Presence by Default

Normal contact UI should not expose:

```text
Alice's phone online
Alice's laptop away
```

unless advanced security/device detail is intended.

---

# 15. Presence Locations

Appropriate surfaces:

```text
contact profile
conversation header
small contact row indicator
group member list for small groups
```

---

# 16. Presence in Inbox

Optional small indicator.

Must not reorder conversations.

---

# 17. Presence in Conversation Header

Good place for:

```text
Online
Away
Last seen recently
```

---

# 18. Presence in Group

Small groups may show per-member presence.

Large groups should avoid full live-state rendering.

---

# 19. Large Group Presence

Possible summary:

```text
12 active
```

or omit entirely.

---

# 20. Presence Dot Semantics

If using a dot:

```text
dot + text/tooltip
```

not color-only meaning.

---

# 21. Availability State

Recommended:

```rust
pub enum AvailabilityState {
    Available,
    Busy,
    DoNotDisturb,
    Invisible,
}
```

---

# 22. Invisible

User can appear:

```text
Unknown
```

to others while still receiving messages.

---

# 23. Do Not Disturb

Affects:

```text
presence/status
notification policy
```

but not necessarily message delivery.

---

# 24. Busy

Informational.

May influence call UI:

```text
Busy
```

but does not necessarily block calls.

---

# 25. Status Text

Optional short user-defined status:

```text
In class
Working
Back later
```

---

# 26. Status Text Privacy

Treat as profile content.

Do not over-index/share beyond intended audience.

---

# 27. Status Expiry

Optional:

```text
Clear after 1 hour
Clear tonight
Clear manually
```

---

# 28. Presence Settings

Recommended:

```text
Show when online
Show last seen
Share typing indicators
Share read receipts
Show status
```

---

# 29. Typing Indicator

Ephemeral.

```rust
pub struct TypingSummary {
    pub participants: Vec<AccountId>,
    pub state: TypingState,
}
```

---

# 30. Typing States

```rust
pub enum TypingState {
    None,
    Typing,
    Recording,
}
```

For multiple users, aggregate separately if needed.

---

# 31. Typing Start

Triggered by meaningful input.

Do not start on:

```text
opening composer
moving cursor
opening attachment picker
```

---

# 32. Typing Stop

On:

```text
idle timeout
send
clear
leave conversation
focus loss
privacy disable
```

---

# 33. Typing TTL

Rust owns.

If stop packet is lost, TTL clears stale indicator.

---

# 34. Typing Frequency

Rate-limit updates.

Example:

```text
start
periodic keepalive
stop
```

not per keystroke.

---

# 35. 1:1 Typing UX

Show:

```text
Alice is typing…
```

---

# 36. Group Typing UX

Examples:

```text
Alice is typing…
Alice and Bob are typing…
Several people are typing…
```

---

# 37. Typing Participant Limit

Do not list 20 names.

---

# 38. Typing Placement

Best:

```text
near bottom of timeline / above composer
```

---

# 39. Typing Does Not Become Timeline Row

Hard rule.

---

# 40. Typing While User Reads History

Indicator can remain near composer.

Do not scroll timeline.

---

# 41. Recording Indicator

If peer is recording voice note:

```text
Alice is recording audio…
```

---

# 42. Recording Indicator Privacy

Same privacy control as typing or separate advanced setting.

---

# 43. Attachment Preparation

Do not show:

```text
Alice is typing…
```

just because peer selected a file.

---

# 44. Delivery State

Outgoing message states:

```rust
pub enum MessageDeliveryState {
    Queued,
    Sending,
    Sent,
    Delivered,
    Read,
    Failed,
}
```

---

# 45. Queued

Meaning:

```text
durably accepted locally, waiting for route/outbox
```

---

# 46. Sending

Actively attempting transfer.

---

# 47. Sent

Sender-side protocol state completed.

Exact semantics documented by backend.

---

# 48. Delivered

Recipient side durably accepted according to protocol.

---

# 49. Read

Recipient read cursor advanced through message.

---

# 50. Failed

Requires attention or retry.

---

# 51. Delivery Icon UX

Possible compact visual states:

```text
clock
single check
double check
read marker
warning
```

But each must have accessible text.

---

# 52. Do Not Copy Familiar Icon Semantics Blindly

If backend meaning differs, labels/details must match actual semantics.

---

# 53. Message Details

On demand show:

```text
Queued at
Sent at
Delivered at
Read at
```

where available.

---

# 54. Group Delivery

Avoid per-message crowded member lists.

Use aggregate:

```text
Delivered to 5
Read by 3
```

---

# 55. Group Receipt Details

Dedicated sheet/dialog can show participants for small groups.

---

# 56. Large Group Receipts

Possible policy:

```text
aggregate only
disabled
```

to reduce cost/privacy.

---

# 57. Read Cursor

Recommended protocol model:

```rust
pub struct ReadCursor {
    pub conversation: ConversationId,
    pub through: MessageSequence,
}
```

---

# 58. Read Cursor Advantage

Avoids sending one receipt per message.

---

# 59. Read Receipt Semantics

If cursor is through sequence 120:

```text
messages <= 120 are read
```

subject to visibility/security policy.

---

# 60. Read Detection — Desktop

Suggested criteria:

```text
conversation active
window focused
message sufficiently visible
```

---

# 61. Read Detection — Android

Suggested:

```text
Activity resumed
conversation route active
message visible
```

---

# 62. Opening Conversation Is Not Enough

Do not mark all unread messages read immediately on navigation.

---

# 63. Scrolling Through Unread

Advance cursor to highest actually viewed eligible sequence.

---

# 64. New Message at Bottom

If user is focused and near bottom:

```text
may become read quickly
```

after visibility criteria.

---

# 65. New Message While Scrolled Up

Do not mark read.

---

# 66. Notification Is Not Read

Showing notification does not advance cursor.

---

# 67. Notification Tap

Opening conversation may advance read later based on viewport.

---

# 68. Mark Read Action

Explicitly advances cursor according to product semantics.

---

# 69. Mark Unread

As Part 04:

```text
local reminder state
```

not undo of remote read receipt.

---

# 70. Read Receipt Privacy

User setting may disable sending read receipts.

---

# 71. When Read Receipts Disabled

Local unread/read still works.

Remote peer does not receive read cursor.

---

# 72. Incoming Read Receipt

If peer disabled read receipts:

```text
Delivered
```

may remain highest visible state.

Do not imply not read.

---

# 73. Unknown vs Not Read

Important wording.

No receipt means:

```text
read status unavailable
```

not proof of unread.

---

# 74. Group Read Privacy

May be configurable separately.

---

# 75. Read Receipt in Requests

Default:

```text
not sent
```

until request accepted.

---

# 76. Typing in Requests

Default:

```text
not sent
```

---

# 77. Presence in Requests

Default:

```text
not shared
```

---

# 78. Blocked Contact

Presence/typing/read sharing stops according to policy.

---

# 79. Muted Conversation

Mute affects notifications.

It should not necessarily affect:

```text
presence
typing
read receipts
```

unless user privacy setting says.

---

# 80. Offline State

Conversation header may show:

```text
Offline
```

only if app itself has no usable path.

Peer-specific state should use `Unknown` if uncertain.

---

# 81. Global Connectivity vs Peer Presence

Separate.

```text
App offline
```

is local connectivity.

```text
Alice unknown
```

is peer presence.

---

# 82. Presence During Local LAN

Peer can be:

```text
Online nearby
```

even without Internet.

Do not equate Internet with online.

---

# 83. Presence over DTN

Presence may be intentionally suppressed/stale.

UI should avoid pretending liveness.

---

# 84. Last Seen over Delayed Transport

If updates delayed, confidence may degrade to:

```text
recently
```

---

# 85. Presence Hysteresis

Rust prevents flicker:

```text
Online
Unknown
Online
Unknown
```

during short network changes.

---

# 86. UI Stability

Presence change should re-render only:

```text
header
row indicator
member row
```

---

# 87. Typing Update Locality

Only bottom typing region / relevant row.

---

# 88. Read Receipt Update Locality

Only affected outgoing message group/details.

---

# 89. Multi-Device Read Aggregation

User's own devices may read at different positions.

Account-level read cursor can be:

```text
max securely synchronized cursor
```

if backend chooses.

UI consumes result.

---

# 90. Multi-Device Typing

If same user types on multiple devices:

```text
one account-level typing state
```

not duplicate:

```text
Alice is typing twice
```

---

# 91. Multi-Device Presence

Same.

---

# 92. Read on Another Device

Local unread badge updates.

Do not move current scroll position.

---

# 93. Message Read on Phone, Desktop Open

Desktop can update:

```text
read cursor
unread separator
inbox badge
```

without forcibly jumping.

---

# 94. Read Receipt Event

```rust
pub struct ReceiptUpdateView {
    pub conversation: ConversationId,
    pub participant: AccountId,
    pub delivered_through: Option<MessageSequence>,
    pub read_through: Option<MessageSequence>,
}
```

---

# 95. Delivery Cursor

Protocol may aggregate delivery similarly.

---

# 96. Receipt Timestamp

If exact timestamp exists, details can show.

---

# 97. Time Precision

Avoid implying exactness if backend timestamp uncertain.

---

# 98. Status/Presence in Contact List

Use subtle:

```text
Online
Away
```

Only if user has enabled display.

---

# 99. Status in Conversation Header

Preferred place for richer:

```text
Online
Busy
Last seen recently
```

---

# 100. Status in Group Header

Avoid individual status.

Maybe:

```text
5 active
```

---

# 101. DND Status

If peer exposes DND:

```text
Do not disturb
```

may inform user, but should not block send.

---

# 102. Busy Status

Likewise.

---

# 103. Invisible Status

Remote users see:

```text
Unknown
```

not:

```text
Invisible
```

because invisible is private state.

---

# 104. Own Presence Controls

Profile/status menu can expose:

```text
Available
Busy
Do Not Disturb
Invisible
```

---

# 105. Automatic Away

Rust/platform can set away after:

```text
desktop idle
mobile inactivity/background
```

according to policy.

---

# 106. Manual Status Overrides

User-set Busy/DND can override automatic presence.

---

# 107. Status Expiry

Allow:

```text
Until changed
1 hour
Today
```

---

# 108. DND Integration

Application DND may also adjust:

```text
notification behavior
```

but OS DND remains separate.

---

# 109. Do Not Confuse App DND with OS DND

Settings should name scope.

---

# 110. Desktop Status Menu

Can live in profile/avatar menu.

---

# 111. Android Status Menu

Profile/settings bottom sheet.

---

# 112. Custom Status

Optional v1/later.

Could include:

```text
emoji + text
```

---

# 113. Custom Status Safety

Bound length.

No rich HTML.

---

# 114. Status Expiration Event

Rust clears.

UI refreshes.

---

# 115. Presence Privacy Matrix

Potential:

```text
Online status: Contacts / Nobody
Last seen: Contacts / Nobody
Typing: On / Off
Read receipts: On / Off
```

---

# 116. Fine-Grained Per-Contact Overrides

Future.

Start with global settings.

---

# 117. Managed Enterprise Policy

Could enforce:

```text
read receipts disabled
presence limited
```

UI displays policy-locked setting.

---

# 118. Privacy Setting Sync

Can be account-wide.

---

# 119. OS-Specific Availability

Automatic away logic may differ by platform.

Rust receives normalized lifecycle/activity hints.

---

# 120. Android Background

Do not advertise:

```text
Online
```

forever merely because foreground service exists.

Presence policy decides.

---

# 121. Active Call Presence

Optional account availability:

```text
Busy
```

during call.

---

# 122. Call Status Exposure

Could show:

```text
In a call
```

only if user chooses.

Default may remain private.

---

# 123. Desktop Idle

Platform adapter may report:

```text
active
idle
locked
```

where available.

---

# 124. Device Locked

Could transition:

```text
Away
```

without revealing lock state.

---

# 125. Typing Privacy During Lock

Stop typing immediately on lock/background.

---

# 126. Recording Privacy During Interruption

Stop recording indicator when recording stops/pauses.

---

# 127. Typing Reliability

Missing stop message solved by TTL.

---

# 128. Typing Delay

May wait small threshold before advertising to avoid flicker:

```text
~300–500 ms
```

optional.

---

# 129. Typing Keepalive

Low frequency.

---

# 130. Battery Sensitivity

Presence/typing updates should be lightweight.

---

# 131. Offline Queue

Do not queue stale typing indicators for later delivery.

Hard rule.

---

# 132. Do Not Store Typing Durably

Except maybe transient in-memory diagnostics.

---

# 133. Do Not Backup Presence

Part 33 excludes ephemeral state.

---

# 134. Do Not Search Presence

Part 11 indexes durable content, not typing/presence.

---

# 135. Receipt Durability

Read/delivery cursors are durable/syncable state.

---

# 136. Presence Durability

Usually not.

Last-seen may be retained according to privacy policy.

---

# 137. Event Model

```rust
pub enum PresenceUiEvent {
    PresenceChanged {
        account: AccountId,
        presence: PresenceSummary,
    },
    AvailabilityChanged {
        account: AccountId,
        availability: AvailabilitySummary,
    },
    TypingChanged {
        conversation: ConversationId,
        typing: TypingSummary,
    },
    ReceiptChanged(ReceiptUpdateView),
}
```

---

# 138. Presence Summary

```rust
pub struct PresenceSummary {
    pub state: PresenceState,
    pub last_seen: Option<LastSeenView>,
}
```

---

# 139. Availability Summary

```rust
pub struct AvailabilitySummary {
    pub state: AvailabilityState,
    pub custom_status: Option<StatusTextView>,
}
```

---

# 140. Typing Participant View

```rust
pub struct TypingParticipantView {
    pub account: AccountId,
    pub kind: TypingKind,
}
```

---

# 141. Typing Kind

```rust
pub enum TypingKind {
    Text,
    VoiceRecording,
}
```

---

# 142. Read Receipt Presentation

```rust
pub struct MessageReceiptView {
    pub delivery: MessageDeliveryState,
    pub delivered_at: Option<Timestamp>,
    pub read_at: Option<Timestamp>,
}
```

---

# 143. Group Receipt Summary

```rust
pub struct GroupReceiptSummaryView {
    pub delivered_count: u32,
    pub read_count: u32,
    pub total_eligible: u32,
}
```

---

# 144. Presence Presentation API

```rust
pub trait PresencePresentation {
    async fn contact_presence(
        &self,
        account: AccountId,
    ) -> Result<PresenceSummary, UiError>;

    async fn visible_contacts(
        &self,
        accounts: Vec<AccountId>,
    ) -> Result<Vec<PresenceSummary>, UiError>;

    async fn set_availability(
        &self,
        availability: AvailabilityState,
    ) -> Result<(), UiError>;
}
```

---

# 145. Typing Presentation API

```rust
pub trait TypingPresentation {
    async fn set_typing(
        &self,
        conversation: ConversationId,
        kind: TypingKind,
        active: bool,
    ) -> Result<(), UiError>;
}
```

---

# 146. Receipt Presentation API

```rust
pub trait ReceiptPresentation {
    async fn mark_read_through(
        &self,
        conversation: ConversationId,
        sequence: MessageSequence,
    ) -> Result<(), UiError>;

    async fn message_receipt(
        &self,
        message: MessageId,
    ) -> Result<MessageReceiptView, UiError>;

    async fn group_receipts(
        &self,
        message: MessageId,
    ) -> Result<GroupReceiptSummaryView, UiError>;
}
```

---

# 147. Privacy Settings API

```rust
pub trait PresencePrivacyPresentation {
    async fn settings(
        &self,
    ) -> Result<PresencePrivacySettingsView, UiError>;

    async fn update(
        &self,
        update: PresencePrivacyUpdate,
    ) -> Result<(), UiError>;
}
```

---

# 148. Presence Privacy Settings

```rust
pub struct PresencePrivacySettingsView {
    pub share_online_status: bool,
    pub share_last_seen: bool,
    pub share_typing: bool,
    pub share_read_receipts: bool,
}
```

---

# 149. Android ViewModel

Owns:

```text
visible screen state
status picker
privacy settings presentation
read-visibility reporting
```

Rust owns:

```text
presence truth
TTL
aggregation
receipt cursors
```

---

# 150. Dioxus Presenter

Owns:

```text
status menu
visible-row subscription hints
focus/visibility reporting
receipt details dialog
```

---

# 151. Visible Contact Optimization

UI may report:

```text
currently visible AccountIds
```

so Rust/network can prioritize presence updates.

---

# 152. Do Not Create One Network Subscription Per Row

Aggregate.

---

# 153. Conversation Visibility Reporting

UI sends:

```text
conversation active
visible message range
focus/lifecycle state
```

to read-detection logic.

---

# 154. Read Visibility Event

Potential:

```rust
pub struct ConversationViewportView {
    pub conversation: ConversationId,
    pub first_visible: MessageSequence,
    pub last_visible: MessageSequence,
    pub focused: bool,
}
```

---

# 155. Rust Decides Read Advancement

UI provides observations.

Rust applies policy.

---

# 156. Why This Matters

Prevents Dioxus and Android from implementing subtly different receipt semantics.

---

# 157. Status Formatting

UI localizes:

```text
Online
Away
Busy
Do not disturb
Last seen recently
```

---

# 158. Timestamp Formatting

UI uses local locale/timezone.

---

# 159. Presence Does Not Reorder Lists

Hard rule.

---

# 160. Typing Does Not Reorder Lists

Hard rule.

---

# 161. Read Receipt Does Not Reorder Timeline

Hard rule.

---

# 162. Accessibility — Presence

Screen reader:

```text
Alice, online
```

or:

```text
Alice, status unavailable
```

---

# 163. Typing Accessibility

Can announce:

```text
Alice is typing
```

carefully.

Avoid repeated announcements every keepalive.

---

# 164. Recording Accessibility

```text
Alice is recording a voice message
```

---

# 165. Receipt Accessibility

Outgoing message:

```text
Delivered
Read
Failed
```

---

# 166. Group Receipt Accessibility

```text
Read by 3 of 5
```

---

# 167. Status Picker Accessibility

All states explicitly labeled.

---

# 168. Color Independence

Presence dots/status icons always paired with accessible text/labels.

---

# 169. Large Font

Header/status text wraps/truncates safely.

---

# 170. RTL

Status/typing strings localized and laid out correctly.

---

# 171. Reduced Motion

Typing animation can use static text.

---

# 172. Screen Reader Live Regions

Use only for meaningful changes.

Do not announce presence flapping.

---

# 173. Privacy Test Matrix

Verify:

```text
typing off
read receipts off
presence off
last seen off
invisible
blocked contact
message request
```

---

# 174. Presence Test Matrix

```text
online
away
unknown
stale
multi-device
LAN-only
Internet offline
device sleep
```

---

# 175. Typing Test Matrix

```text
start
keepalive
stop
TTL expiry
group aggregation
background
crash
privacy off
```

---

# 176. Receipt Test Matrix

```text
queued
sent
delivered
read
failed
other-device read
group aggregate
read disabled
notification tap
mark unread
```

---

# 177. Android Tests

Verify:

```text
Activity resumed/paused
screen rotation
process death
background
large font
TalkBack
typing stop on background
read viewport semantics
```

---

# 178. Desktop Tests

Verify:

```text
window focused
unfocused
hidden
multi-window
visible-row presence
keyboard accessibility
```

---

# 179. Multi-Window Read

If same conversation open in two desktop windows:

```text
account/device read state
```

should not double-send harmful events.

---

# 180. Multi-Device Typing Test

Typing from phone and desktop aggregates to one user state.

---

# 181. Presence Flap Test

Short path changes do not flicker status due to hysteresis.

---

# 182. Stale Last-Seen Test

Wording coarsens as confidence decreases.

---

# 183. Large Group Test

Thousands of members do not create thousands of live UI subscriptions.

---

# 184. Burst Receipt Test

Many receipt updates coalesce.

---

# 185. Performance

Presence updates should be low frequency.

Typing updates sparse.

Read cursors aggregated.

---

# 186. Battery

Avoid continuous background presence chatter.

Use adaptive cadence.

---

# 187. Network

Presence is lower priority than:

```text
calls
message delivery
security
emergency traffic
```

---

# 188. Emergency Mode

Presence may be disabled/deprioritized to preserve bandwidth.

---

# 189. Diagnostics

Advanced view may show:

```text
presence age
last update source
receipt cursor
typing TTL
```

with privacy-safe identifiers.

---

# 190. No Raw Transport Data in Normal UX

No:

```text
gossip heartbeat 9s
BLE presence packet
```

---

# 191. Telemetry

Do not log:

```text
who is online
who is typing
who read which message
```

as product analytics by default.

---

# 192. Safe Metrics

Possible:

```text
presence event rate
typing event rate
receipt propagation latency
stale-expiry count
```

without identities/content.

---

# 193. Crash Reports

Redact contact IDs where possible.

---

# 194. Initial Production Scope

Ship:

```text
Online/Away/Unknown
optional last seen
typing
voice-recording indicator
queued/sent/delivered/read/failed
read cursor
presence/read/typing privacy settings
account-level multi-device aggregation
small-group typing aggregation
large-group receipt aggregation
```

Defer:

```text
rich custom status
per-contact privacy matrix
presence location
device-specific presence display
activity/game/music status
```

unless needed.

---

# 195. Definition of Done

UI/UX Part 14 is complete when:

- presence uses Online/Away/Unknown rather than pretending unknown means offline
- account-level presence aggregates multiple devices in Rust
- availability is distinct from presence
- last-seen privacy and precision are explicit
- typing/recording indicators are ephemeral and TTL-controlled by Rust
- typing/presence never reorder inbox/timeline
- read receipts use durable sequence/cursor semantics
- opening a conversation alone does not automatically mark everything read
- notification display/dismissal does not imply read
- mark-unread remains a local reminder rather than reversing remote receipts
- delivery states are semantically defined
- absence of read receipt is not presented as proof of unread
- requests/blocked/privacy modes suppress ephemeral sharing appropriately
- large groups use aggregation rather than per-member live UI
- Android/desktop viewport/focus observations feed one shared Rust read policy
- accessibility, RTL, large font, reduced motion, and live-region rules are defined
- Rust presence, typing, receipt, and privacy presentation APIs are specified
- multi-device, TTL, offline, lifecycle, privacy, flap, and scale tests are included

---

# 196. Final Architecture

```text
                  RUST EPHEMERAL STATE
                          │
       ┌──────────────────┼──────────────────┐
       │                  │                  │
    Presence           Typing            Receipts
       │                  │                  │
   TTL/Hysteresis      TTL/Rate Limit    Durable Cursor
       │                  │                  │
       └──────────────────┼──────────────────┘
                          │
               Presence Presentation
                    ┌─────┴─────┐
                    │           │
                 Dioxus      Compose
                    │           │
             Desktop UX     Android UX
```

UI observations flow back only as:

```text
focus
lifecycle
visible message range
visible contact set
```

Rust remains responsible for deciding:

```text
presence
typing expiry
read advancement
receipt semantics
```

---

# 197. Final Principle

Presence and receipts should feel informative, not invasive or falsely precise.

The correct model is:

```text
useful hints
+
clear uncertainty
+
durable cursor semantics
+
privacy controls
+
bounded ephemeral updates
```

not:

```text
every device heartbeat becomes user-visible truth
```

This keeps the experience calm and trustworthy across Dioxus desktop and Android Compose while Rust remains authoritative for ephemeral state aggregation and receipt semantics.
