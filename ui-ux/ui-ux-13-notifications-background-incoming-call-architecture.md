# UI/UX Part 13 — Notifications, Background & Incoming Call UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 13  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define message, call, security, transfer, emergency, background wake, notification privacy, grouping, deduplication, deep-linking, foreground-vs-background behavior, and platform notification UX across desktop and Android.

---

# 1. Purpose

Notifications sit at the boundary between the communication product and the operating system.

They must work correctly when:

```text
the app is foregrounded
the app is backgrounded
the Activity is destroyed
the desktop window is hidden
the desktop UI is closed but daemon remains
the device is locked
notification permission is denied
multiple devices receive the same event
a call arrives
a message arrives while viewing another conversation
a message arrives while already viewing that conversation
a security event occurs
a transfer finishes
emergency traffic arrives
```

The governing principle is:

> **Rust decides notification meaning and privacy; platform code decides how that meaning is presented through the operating system.**

---

# 2. Architectural Position

```text
Rust Event / Notification Policy
            │
            ├── semantic event
            ├── urgency
            ├── privacy class
            ├── dedup key
            ├── actions
            └── destination
            │
            ▼
Notification Presentation Service
            │
      ┌─────┴─────┐
      │           │
 Desktop       Android
 Adapter        Adapter
      │           │
 Native OS     NotificationManager
 Notification  Foreground Service
 Tray          Full-screen call surface
```

---

# 3. Notification Is Not Domain Truth

A notification is a projection.

Deleting/dismissing it must not delete:

```text
message
call history
security event
transfer record
```

unless the notification action explicitly invokes such a command.

---

# 4. Notification Categories

Recommended:

```rust
pub enum NotificationCategory {
    Message,
    Mention,
    MessageRequest,
    IncomingCall,
    MissedCall,
    Security,
    Transfer,
    Backup,
    Emergency,
    System,
}
```

---

# 5. Notification Priority

```rust
pub enum NotificationUrgency {
    Passive,
    Normal,
    High,
    Critical,
}
```

Examples:

```text
Passive  → transfer complete
Normal   → ordinary message
High     → mention / incoming call
Critical → emergency / severe security event
```

---

# 6. Privacy Classes

```rust
pub enum NotificationPrivacy {
    Full,
    SenderOnly,
    Generic,
    Hidden,
}
```

---

# 7. Full Preview

May show:

```text
Alice
See you at 5 PM
```

---

# 8. Sender-Only Preview

Shows:

```text
Alice
New message
```

---

# 9. Generic Preview

Shows:

```text
New message
```

without identity.

---

# 10. Hidden

No content-bearing notification.

May still update badge/count depending settings.

---

# 11. Lockscreen Privacy

Platform adapter combines:

```text
Rust privacy policy
+
OS lockscreen policy
+
user setting
```

using the strictest effective result.

---

# 12. App Foreground State

Rust/platform tracks semantic presentation state:

```text
Foreground
Background
Hidden
NoUI
Locked
```

---

# 13. Focused Conversation Suppression

If user is actively viewing the same conversation and relevant message is visible:

```text
do not show redundant OS notification
```

Use:

```text
in-app update
optional subtle banner
```

---

# 14. Different Conversation While Foreground

If user is in app but another conversation receives a message:

```text
in-app banner
```

can be preferred over OS notification.

---

# 15. Background

Use OS notification according to policy.

---

# 16. Desktop Hidden to Tray

Treat as background for notification purposes.

---

# 17. Desktop Main Window Visible but Unfocused

Usually OS/native notification may still be useful.

Policy can distinguish:

```text
visible + focused
visible + unfocused
hidden
```

---

# 18. Notification Policy Input

```rust
pub struct NotificationContext {
    pub app_visibility: AppVisibility,
    pub active_destination: Option<UiDestination>,
    pub device_locked: bool,
    pub notification_permission: NotificationPermissionState,
}
```

---

# 19. Notification Intent

```rust
pub struct NotificationIntent {
    pub id: NotificationId,
    pub category: NotificationCategory,
    pub urgency: NotificationUrgency,
    pub privacy: NotificationPrivacy,
    pub title: NotificationText,
    pub body: Option<NotificationText>,
    pub badge_delta: Option<i32>,
    pub destination: Option<UiDestination>,
    pub actions: Vec<NotificationActionView>,
    pub dedup_key: NotificationDedupKey,
}
```

---

# 20. Notification Text

Rust should provide semantic/localizable payloads where possible.

Do not force English strings from core.

Example:

```rust
pub enum NotificationText {
    MessageFrom { sender: DisplayName },
    NewMessage,
    MissedCallFrom { sender: DisplayName },
    SecurityAlert,
}
```

Platform/UI localizes.

---

# 21. Android Notification Channels

Recommended stable channels:

```text
Messages
Mentions
Calls
Security
Transfers
Emergency
Background/System
```

---

# 22. Channel Stability

Once created, Android channel identity should remain stable across updates.

---

# 23. User-Controlled Channel Settings

Android users may change:

```text
sound
vibration
importance
lock-screen visibility
```

The app should respect system ownership.

---

# 24. Android Notification Permission

Request contextually.

Do not request on first launch solely because permission exists.

---

# 25. Recommended Permission Timing

Ask when user enables or first meaningfully needs:

```text
message alerts
background calling
```

with explanation.

---

# 26. Permission Denied

App remains usable.

Explain:

```text
You may miss message and call alerts while the app is not open.
```

---

# 27. Permanently Denied

Offer:

```text
Open Settings
```

---

# 28. Notification Permission Diagnostics

Settings/diagnostics can show:

```text
Allowed
Denied
Blocked by system
```

---

# 29. Desktop Notification Permissions

Where desktop environment supports permission controls, adapter reports capability/state.

---

# 30. Message Notification

Normal message notification may show:

```text
sender
conversation
preview
```

according to privacy mode.

---

# 31. Group Message Notification

Possible title:

```text
Alice · Project Group
```

body:

```text
New deadline is Friday
```

---

# 32. Mention Notification

May have higher urgency than normal group message.

---

# 33. Muted Conversation

No ordinary message notification.

Mentions may bypass only if user setting permits.

---

# 34. Message Request Notification

Privacy-conscious:

```text
New message request
```

rather than automatically revealing unknown sender/content.

---

# 35. Unknown Attachment Notification

Do not show risky filename/content preview if privacy/security policy disallows.

---

# 36. Notification Grouping — Android

Group by conversation where possible.

Example:

```text
Project Group
  Alice: ...
  Bob: ...
```

---

# 37. Notification Summary

For many conversations:

```text
5 new messages from 3 conversations
```

---

# 38. Desktop Grouping

Native environment dependent.

If unsupported, keep dedup/burst control in application layer.

---

# 39. Notification Burst Coalescing

Rapid messages from same conversation should not produce disruptive sound for every event.

---

# 40. Burst Policy

Example:

```text
first message → sound
subsequent messages within short window → update existing notification
```

---

# 41. Deduplication

Every logical notification event has stable dedup key.

```rust
pub enum NotificationDedupKey {
    Message(MessageId),
    Call(CallId),
    Security(SecurityEventId),
    Transfer(TransferId),
    Backup(BackupJobId),
    Emergency(EmergencyEventId),
}
```

---

# 42. Duplicate Delivery Paths

If same message arrives via:

```text
direct
relay
DTN
```

only one notification.

---

# 43. Multi-Device Notification Semantics

Each user device may notify independently.

Cross-device suppression is optional advanced policy.

---

# 44. Read on Another Device

If message becomes read elsewhere:

```text
local notification may be dismissed/updated
```

if synchronization arrives.

---

# 45. Answered Elsewhere

Incoming call notification must disappear when another device answers.

---

# 46. Declined Elsewhere

Policy may dismiss other ringing devices.

---

# 47. Deep-Link Routing

Notification action never routes using raw unvalidated string.

Flow:

```text
notification
→ typed destination
→ Rust validates target still exists/authorized
→ navigate
```

---

# 48. Message Destination

```text
Conversation(ConversationId)
```

optionally with:

```text
MessageId
```

---

# 49. Call Destination

```text
Call(CallId)
```

---

# 50. Security Destination

```text
SecurityEvent(SecurityEventId)
```

---

# 51. Transfer Destination

```text
Transfer(TransferId)
```

---

# 52. Cold Start — Android

Tap notification with app dead:

```text
Activity starts
→ Rust runtime initializes
→ launch action retained
→ core Ready
→ validate destination
→ navigate
```

---

# 53. Pending Launch Action

```rust
pub enum PendingLaunchAction {
    OpenConversation {
        conversation: ConversationId,
        message: Option<MessageId>,
    },
    OpenCall(CallId),
    OpenSecurityEvent(SecurityEventId),
    OpenTransfer(TransferId),
    OpenEmergency(EmergencyEventId),
}
```

---

# 54. Stale Destination

If target no longer exists:

```text
show safe fallback
```

Example:

```text
Message is no longer available
```

---

# 55. Android Background Wake

A push/wake signal is not trusted message content.

Correct flow:

```text
Push wake
→ wake/schedule Rust
→ authenticate/fetch/process event
→ decide notification
```

---

# 56. Do Not Render Push Payload Directly

Hard rule for sensitive events.

---

# 57. Push as Hint

Push may contain:

```text
opaque wake token
account/device routing hint
```

but not authoritative message body.

---

# 58. Background Catch-Up

When device wakes:

```text
sync/fetch pending events
persist durably
evaluate notification policy
```

---

# 59. UI Not Required

Background receive must work without Compose Activity.

---

# 60. Background Event Persistence

Message is persisted before notification if possible.

Then notification tap always has durable target.

---

# 61. Background Retry

If wake cannot complete:

```text
retry according to platform policy
```

without duplicate notification.

---

# 62. Android Process Death

Notification tap launches fresh process and reconstructs state from Rust durability.

---

# 63. Desktop Daemon Mode

Daemon can receive events and ask notification adapter to alert even if Dioxus UI is closed.

---

# 64. Desktop Embedded Mode

If core dies with UI, no background notifications until app running.

Product should communicate this in settings if relevant.

---

# 65. System Tray

Recommended desktop actions:

```text
Open App
New Message
Active Call
Mute Notifications temporarily
Quit
```

---

# 66. Tray Badge

Optional unread count.

---

# 67. Dock/Taskbar Badge

Can show unread aggregate where platform supports.

---

# 68. Badge Count

Derived from durable unread state.

Not independently incremented by notification events.

---

# 69. Mark Read from Notification

Optional action:

```text
Mark Read
```

Rust advances local/account read state according to semantics.

---

# 70. Reply from Notification

Future optional.

If implemented:

```text
text input
→ authenticated app/service
→ Rust SendMessageCommand
```

No direct OS-to-network path bypassing Rust.

---

# 71. Notification Reply Security

Respect:

```text
device lock
app lock
privacy setting
```

May disable quick reply on lock screen.

---

# 72. Incoming Call — Android

Critical flow:

```text
wake signal
→ Rust authenticates call offer
→ CallState = Incoming
→ Android call notification / full-screen surface
```

---

# 73. Do Not Ring on Unauthenticated Push

Hard rule.

---

# 74. Incoming Call Notification

Shows:

```text
caller identity
Audio call / Video call
Accept
Decline
```

according to privacy policy.

---

# 75. Full-Screen Incoming Call

Use only where Android platform policy permits and product qualifies.

Otherwise:

```text
high-priority heads-up notification
```

---

# 76. Lock-Screen Call

Must respect privacy setting.

Strict mode:

```text
Incoming call
```

without caller identity.

---

# 77. Accept from Notification

Flow:

```text
PendingIntent
→ service/activity receiver
→ Rust CallController.accept(CallId)
→ foreground call service
→ optional full call UI
```

---

# 78. Decline from Notification

Can execute without launching full Activity where platform allows.

---

# 79. Dismiss Incoming Call Notification

Do not necessarily equal decline.

Recommendation:

```text
swipe/dismiss → silence local alert
```

while backend call remains until timeout unless product explicitly maps dismissal to decline.

---

# 80. Silence Call

Optional action:

```text
Silence
```

without rejecting peer.

---

# 81. Missed Call

After timeout/no answer:

```text
Missed call from Alice
```

with:

```text
Call back
Message
```

where allowed.

---

# 82. Call Answered Elsewhere

Remove missed-call possibility on current device if another device answered.

---

# 83. Desktop Incoming Call

If app visible:

```text
in-app call surface
```

plus optional native notification if unfocused.

If hidden:

```text
native notification
+
ring sound
+
optional floating incoming-call window
```

---

# 84. Desktop Accept

Raises/creates call window.

---

# 85. Desktop Decline

No need to open main app.

---

# 86. Ongoing Call Notification — Android

Foreground service notification persists while call active.

---

# 87. Ongoing Call Actions

Potential:

```text
Mute
Hang Up
Return to Call
```

---

# 88. Ongoing Call Privacy

Lock-screen content can be generic.

---

# 89. Active Call + Incoming Message

Do not disrupt with loud notification by default.

Possible:

```text
quiet heads-up/banner
```

---

# 90. Call Priority

Realtime call audio has priority over ordinary notification sounds.

---

# 91. Notification Sound During Call

Prefer:

```text
soft/none
```

for ordinary messages.

Emergency/security policy may override.

---

# 92. Transfer Notifications

Notify only when useful.

Examples:

```text
large transfer completed
transfer failed
waiting for action
```

Do not notify every small auto-downloaded image.

---

# 93. Active Transfer Notification Android

Long user-visible transfer may show ongoing progress.

---

# 94. Transfer Complete

Potential:

```text
File downloaded
Open
```

---

# 95. Transfer Failure

```text
Download failed
Retry
```

if retryable.

---

# 96. Backup Notification

Useful for:

```text
backup failed
backup completed if user initiated
recovery attention required
```

Avoid noise for routine successful background backups unless user opted in.

---

# 97. Security Notification

Examples:

```text
New device linked
Identity changed
Device revoked
Verification problem
Suspicious link attempt
```

---

# 98. Security Urgency

Meaningful security events should not be hidden inside ordinary message channel.

Use dedicated Security channel/category.

---

# 99. Security Preview

May intentionally show generic:

```text
Security alert
Open the app to review
```

on lock screen.

---

# 100. Emergency Notifications

Part 17.

Critical emergency events may:

```text
override ordinary mute policy
```

only if user/product policy explicitly allows.

---

# 101. Emergency Channel

Dedicated.

User should understand behavior in settings.

---

# 102. Emergency False Positive Prevention

Only authenticated, policy-approved emergency events reach critical notification UX.

---

# 103. Notification Settings

Recommended hierarchy:

```text
Messages
Mentions
Calls
Requests
Security
Transfers
Emergency
Preview Privacy
Sound/Vibration
Quiet Hours
Per-Conversation Overrides
```

---

# 104. Android Settings Relationship

Some sound/importance controls live in Android system channels.

App should link:

```text
Open Android Notification Settings
```

rather than pretending to override system state.

---

# 105. Desktop Settings

App can own more behavior directly.

---

# 106. Per-Conversation Notification Policy

```rust
pub enum ConversationNotificationPolicy {
    All,
    Mentions,
    Muted,
}
```

---

# 107. Mute Duration

Potential:

```text
1 hour
8 hours
1 day
1 week
Forever
```

---

# 108. Quiet Hours

Optional local preference:

```text
start
end
days
exceptions
```

---

# 109. Quiet Hours Exceptions

Potential:

```text
Calls
Security
Emergency
Favorite contacts
Mentions
```

---

# 110. Timezone

Quiet hours use device/local configured time.

---

# 111. Device-Local Policy

Some notification preferences are device-specific.

Examples:

```text
sound
vibration
lock-screen preview
quiet hours
```

---

# 112. Account-Wide Policy

Potentially sync:

```text
conversation mute
mention policy
```

---

# 113. Do Not Sync OS Channel Configuration

Android channel sound/importance is platform/device-local.

---

# 114. Notification Preview Setting

Recommended:

```text
Show sender and message
Show sender only
Show generic notification
```

---

# 115. Lock-Screen Override

Separate:

```text
Hide previews on lock screen
```

---

# 116. App Lock Integration

If app locked:

```text
strictest preview policy
```

can apply.

---

# 117. Screen Sharing Integration

While screen sharing:

```text
temporarily hide notification previews
```

optional privacy setting.

---

# 118. Desktop Screen Share

Could suppress native popup previews while sharing.

---

# 119. Do Not Lose Events

Suppressing notification visual does not suppress durable event.

---

# 120. Foreground In-App Banner

For message in another conversation:

```text
avatar
sender/group
preview
Tap to open
```

---

# 121. Banner Duration

Short and non-blocking.

---

# 122. Banner Stacking

Do not stack dozens.

Queue/coalesce.

---

# 123. In-App Security Banner

May persist until reviewed.

---

# 124. In-App Emergency Banner

May be persistent/high priority.

---

# 125. Notification Center Inside App

Optional future:

```text
Activity / Alerts
```

for security/system events.

Not required for ordinary message notification history.

---

# 126. Desktop Notification Click

Routes to typed destination.

Raises existing single-instance window.

---

# 127. Single-Instance Integration

If desktop app already running:

```text
notification click
→ existing instance
→ typed navigation
```

---

# 128. Desktop UI Closed but Daemon Running

Click notification:

```text
launch/attach UI
→ validate destination
→ open target
```

---

# 129. Notification Action Capability

Rust provides allowed actions.

```rust
pub enum NotificationActionView {
    Open,
    MarkRead,
    Reply,
    AcceptCall,
    DeclineCall,
    RetryTransfer,
    ReviewSecurity,
}
```

---

# 130. UI Must Not Invent Actions

Example:

```text
AcceptCall
```

only if Rust says call is still incoming.

---

# 131. Stale Action

If user taps Accept after call ended:

```text
safe no-op
+
show call ended
```

---

# 132. Idempotency

Notification actions use stable command IDs/CallId/MessageId.

Repeated OS delivery does not duplicate action.

---

# 133. Notification Cancellation

Rust can emit:

```text
Cancel NotificationId
```

when event no longer relevant.

---

# 134. Examples

```text
message read elsewhere
call answered elsewhere
transfer completed and opened
security issue resolved
```

---

# 135. Notification Update

Existing notification can be updated rather than replaced.

---

# 136. Android Group Summary Update

Conversation notification updates with latest messages.

---

# 137. Desktop Burst Update

If native API supports replace-ID, reuse notification identity.

---

# 138. Background Work States

```rust
pub enum BackgroundUiState {
    Idle,
    Syncing,
    Receiving,
    Reconnecting,
    Paused,
    Degraded,
}
```

---

# 139. Normal Background Sync

Usually invisible.

---

# 140. Persistent Background Indicator

Do not keep permanent "syncing" notification for ordinary app operation unless Android foreground-service rules require a visible user-facing operation.

---

# 141. Long-Running User Operation

Examples:

```text
active call
large transfer
explicit backup
```

may justify persistent notification.

---

# 142. Work Manager / Scheduler Boundary

Android scheduling mechanism is platform implementation.

Rust owns semantic job state.

---

# 143. Battery Saver

May delay non-critical background fetch.

UI generally does not warn unless messages are being delayed significantly.

---

# 144. Background Restricted

If Android has heavily restricted app:

```text
Background delivery may be delayed
```

diagnostics/settings can explain.

---

# 145. Battery Optimization Education

Do not aggressively ask user to disable battery optimization on first launch.

Only surface if real delivery problems occur and platform permits guidance.

---

# 146. Push Token State

Not user-facing normally.

Diagnostics can show:

```text
Registered
Unavailable
Error
```

---

# 147. No Push Provider Detail in Normal UX

Users do not need:

```text
FCM token
APNs equivalent
```

in everyday UI.

---

# 148. Background Catch-Up Indicator

On returning after long offline period:

```text
Syncing recent messages…
```

small status if catch-up takes noticeable time.

---

# 149. Local History First

Existing messages render immediately while background catch-up continues.

---

# 150. Notification-to-Read Semantics

Showing a notification does not mean message read.

---

# 151. Notification Tap

Opening conversation may later advance read cursor based on actual visibility.

---

# 152. Notification Dismissal

Does not mark read by default.

---

# 153. Mark Read Action

Explicit action only.

---

# 154. Notification Reply

If implemented, successful send does not necessarily mark all incoming messages read unless policy explicitly says.

---

# 155. Message Request Notification Tap

Opens request view, not trusted conversation directly.

---

# 156. Security Notification Tap

Opens security event details.

---

# 157. Emergency Notification Tap

Opens dedicated emergency surface.

---

# 158. Notification Accessibility

Title/body/actions must be meaningful without relying on icons.

---

# 159. Android TalkBack

Actions:

```text
Accept call
Decline call
Mark read
```

must be clearly labeled.

---

# 160. Desktop Screen Reader

Native notification accessibility is OS-dependent; app in-focus banners must be fully accessible.

---

# 161. In-App Banner Focus

Do not steal keyboard focus for ordinary message banner.

---

# 162. Critical Alert Focus

Security/emergency may request attention but should still avoid disruptive forced focus unless policy demands.

---

# 163. Large Font

In-app banners/settings wrap cleanly.

---

# 164. RTL

Notification localized text and in-app banners support RTL.

---

# 165. Reduced Motion

Banner entrance/exit animations optional.

---

# 166. Color Independence

Urgency/security states use text/icon, not color only.

---

# 167. Notification Sound Accessibility

Visual/vibration alternatives should exist.

---

# 168. Vibration

Respect OS/user settings.

---

# 169. Desktop Quiet Mode

System Do Not Disturb should generally be respected.

---

# 170. Android Do Not Disturb

App must not bypass DND unless explicitly permitted and justified.

---

# 171. Critical/Emergency Override

Any bypass capability must be explicit, narrow, user-controlled, and platform-compliant.

---

# 172. Notification Telemetry

Do not log:

```text
message body
sender name
notification preview text
```

by default.

---

# 173. Safe Metrics

Possible:

```text
notification generated
notification suppressed due to foreground
notification permission state
tap/open latency
call answer latency
```

without identities/content.

---

# 174. Crash Reports

Redact notification contents.

---

# 175. Notification Presentation API

```rust
pub trait NotificationPresentation {
    async fn evaluate(
        &self,
        event: NotificationEvent,
        context: NotificationContext,
    ) -> Result<NotificationDecision, UiError>;

    async fn action(
        &self,
        notification: NotificationId,
        action: NotificationActionView,
    ) -> Result<NotificationActionResult, UiError>;

    async fn dismissed(
        &self,
        notification: NotificationId,
    ) -> Result<(), UiError>;
}
```

---

# 176. Notification Decision

```rust
pub enum NotificationDecision {
    Suppress,
    InApp(InAppNotificationView),
    System(NotificationIntent),
    Both {
        in_app: InAppNotificationView,
        system: NotificationIntent,
    },
}
```

Use `Both` sparingly.

---

# 177. Notification Event

```rust
pub enum NotificationEvent {
    Message(MessageNotificationEvent),
    IncomingCall(IncomingCallNotificationEvent),
    MissedCall(MissedCallNotificationEvent),
    Security(SecurityNotificationEvent),
    Transfer(TransferNotificationEvent),
    Backup(BackupNotificationEvent),
    Emergency(EmergencyNotificationEvent),
}
```

---

# 178. Background Wake API

```rust
pub trait BackgroundWakePresentation {
    async fn process_wake(
        &self,
        wake: BackgroundWakeToken,
    ) -> Result<BackgroundWakeResult, UiError>;
}
```

Platform adapters never interpret private event content before Rust.

---

# 179. Notification Events to Platform

```rust
pub enum NotificationUiEvent {
    Show(NotificationIntent),
    Update(NotificationIntent),
    Cancel(NotificationId),
    BadgeChanged(u32),
}
```

---

# 180. Android Platform Adapter

Responsibilities:

```text
channels
permission
NotificationManager
PendingIntent
foreground service notification
full-screen call eligibility
lock-screen visibility
system badge support
```

---

# 181. Compose ViewModel

Owns:

```text
in-app banners
notification permission education UI
settings navigation effects
```

It does not own OS notification truth.

---

# 182. Desktop Adapter

Responsibilities:

```text
native notifications
tray
taskbar/dock badge
single-instance activation
window raise
```

---

# 183. Dioxus Presenter

Owns:

```text
in-app banner stack
notification settings presentation
tray-related UI state
```

---

# 184. No Direct OS Notification from Domain Actors

Hard boundary:

```text
domain event
→ notification policy
→ platform adapter
```

---

# 185. Notification Settings Snapshot

```rust
pub struct NotificationSettingsView {
    pub message_policy: MessageNotificationPolicy,
    pub call_enabled: bool,
    pub request_enabled: bool,
    pub transfer_policy: TransferNotificationPolicy,
    pub security_enabled: bool,
    pub emergency_policy: EmergencyNotificationPolicy,
    pub preview_policy: NotificationPreviewPolicy,
    pub quiet_hours: Option<QuietHoursView>,
}
```

---

# 186. Per-Conversation Policy

Conversation settings override global message policy within allowed rules.

---

# 187. System-Level Conflict

If app says notifications enabled but Android channel disabled:

```text
show system-disabled state
Open Android Settings
```

---

# 188. Permission Education Screen

Keep concise:

```text
Allow notifications to receive message and call alerts while the app is closed.
```

---

# 189. Incoming Call Reliability Warning

If notification permission/call channel unavailable:

```text
Incoming calls may not alert you when the app is in the background.
```

---

# 190. Diagnostics

Useful status:

```text
Notification permission
Calls channel
Message channel
Push wake state
Background restriction
Foreground call service
Last successful background sync
```

---

# 191. Do Not Show Internal Tokens

No raw push token.

---

# 192. Notification History Debug

Developer mode can show:

```text
NotificationId
category
decision
suppression reason
```

with content redacted.

---

# 193. Suppression Reasons

```rust
pub enum NotificationSuppressionReason {
    ActiveConversationVisible,
    ConversationMuted,
    QuietHours,
    AppLockedPolicy,
    PermissionDenied,
    Duplicate,
    AlreadyRead,
    AnsweredElsewhere,
    PolicyDisabled,
}
```

---

# 194. User-Friendly Diagnostics

Examples:

```text
Muted
Notifications disabled by Android
Suppressed because conversation is open
```

---

# 195. Testing Matrix

Required:

```text
foreground same conversation
foreground different conversation
background message
locked device
muted conversation
mention
message request
incoming call
accept from notification
decline from notification
missed call
answered elsewhere
transfer completion
security alert
emergency alert
permission denied
quiet hours
```

---

# 196. Android Tests

Verify:

```text
runtime notification permission
channel disabled
cold-start notification tap
process death
foreground service call
heads-up call
full-screen call where allowed
lock-screen privacy
background wake
notification action idempotency
```

---

# 197. Desktop Tests

Verify:

```text
focused window suppression
unfocused notification
hidden-to-tray notification
daemon-only delivery
notification click
single-instance activation
tray badge
taskbar/dock badge
```

---

# 198. Duplicate Event Test

Same MessageId via multiple transports:

```text
one notification
```

---

# 199. Multi-Device Read Test

Read on phone:

```text
desktop notification disappears/updates
```

if synchronized before user acts.

---

# 200. Answered Elsewhere Test

Incoming call on two devices.

One answers.

Other notification cancels immediately.

---

# 201. Stale Action Test

Tap Accept after call ended.

No crash/no new call.

---

# 202. Permission Denial Test

App stays usable.

Settings shows degraded background alerts.

---

# 203. Quiet Hours Test

Ordinary message suppressed.

Allowed exception still alerts.

---

# 204. Privacy Test

Strict lock-screen setting never exposes sender/message.

---

# 205. Screen Share Privacy Test

Preview suppression activates/restores correctly.

---

# 206. Background Wake Security Test

Forged push hint cannot produce trusted message/call notification without Rust authentication.

---

# 207. Accessibility Test

Incoming call and message actions are usable with screen reader.

---

# 208. Performance

Notification decision path should be lightweight.

Do not perform expensive search/index operations before ordinary notification.

---

# 209. Burst Performance

100-message burst should:

```text
coalesce
avoid 100 sounds
avoid 100 platform objects
```

---

# 210. Background Energy

Wake only as often as required.

Batch non-urgent catch-up where possible.

---

# 211. Foreground Call Priority

Call media work outranks notification rendering.

---

# 212. Initial Production Scope

Ship:

```text
message notifications
group/mention notifications
message requests
incoming call notification
accept/decline
missed call
security alerts
transfer failure/completion where useful
Android channels + runtime permission
foreground call service notification
desktop native notifications + tray
privacy previews
foreground suppression
deep links
deduplication
quiet/mute policies
```

Defer:

```text
rich inline reply
complex notification history center
cross-device smart suppression heuristics
critical-alert DND bypass
advanced wearable integrations
```

unless explicitly required.

---

# 213. Definition of Done

UI/UX Part 13 is complete when:

- Rust decides notification semantics, privacy, urgency, deduplication, and actions
- platform adapters own OS-specific notification APIs
- same-conversation foreground messages do not produce redundant OS notifications
- message/group/mention/request notification behavior is explicit
- Android channels and runtime notification permission are defined
- lock-screen preview privacy has Full/SenderOnly/Generic/Hidden semantics
- background push/wake data is treated as a hint and authenticated by Rust before notification
- incoming calls never ring solely from unauthenticated push metadata
- accept/decline notification actions route through Rust CallController
- foreground-call service and ongoing-call notification behavior are defined
- desktop focused/unfocused/hidden/daemon notification behavior is defined
- notification clicks use typed validated destinations
- badge counts derive from durable unread state rather than notification counters
- dismissal does not imply read
- muting, mentions, quiet hours, security, transfer, backup, and emergency categories are distinct
- duplicate delivery and multi-device answered/read events can cancel/update notifications
- accessibility, lock-screen privacy, DND respect, RTL, and large font are explicit
- notification content is excluded from telemetry/crash logs by default
- notification/background-wake presentation APIs are defined
- cold-start, process-death, duplicate, stale-action, permission-denied, and privacy tests are specified

---

# 214. Final Architecture

```text
                   RUST DOMAIN EVENTS
                           │
                           ▼
               Notification Policy Engine
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
     Meaning            Privacy           Actions
        │                  │                  │
        └──────────────────┼──────────────────┘
                           │
                  NotificationIntent
                 ┌─────────┴─────────┐
                 │                   │
              Desktop             Android
                 │                   │
       Native Notification   NotificationManager
       Tray / Badge          Channels
       Single Instance       Foreground Service
                             Incoming Call Surface
```

Background receive:

```text
Opaque Wake Hint
      │
      ▼
Rust Authenticate / Sync
      │
      ▼
Persist Durable Event
      │
      ▼
Notification Policy
```

Never:

```text
Push payload
→ directly display private message/call
```

---

# 215. Final Principle

Notifications should be useful without becoming a second, less-secure communication system.

The correct model is:

```text
authenticated durable event
+
Rust privacy/policy decision
+
platform-native presentation
+
typed action/deep-link routing
```

not:

```text
whatever the push payload says is shown to the user
```

This gives Dioxus desktop and Android Compose reliable foreground/background behavior while preserving the Rust core as the source of truth for messages, calls, security, and notification meaning.
