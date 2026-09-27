# UI/UX Part 07 — Calls & Realtime Media UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 07  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete user-facing call and realtime media experience across desktop and Android, including incoming/outgoing call UX, ringing, connecting, active-call controls, audio/video routing, screen sharing, reconnect states, call waiting, hold/resume, Picture-in-Picture, floating desktop call windows, media quality presentation, permissions, accessibility, lifecycle recovery, and the Rust presentation contract that drives both UIs.

---

# 1. Purpose

Realtime communication is one of the most failure-sensitive parts of the product.

A call UI must remain understandable when:

```text
peer is ringing
peer accepts
media initializes slowly
network changes
video fails
audio route changes
camera permission is denied
Bluetooth connects/disconnects
screen sharing starts
call reconnects
another call arrives
app backgrounds
Activity recreates
desktop window closes
```

The governing principle is:

> **The UI represents one logical call session even while transport paths, codecs, surfaces, routes, and platform windows change underneath it.**

---

# 2. Architectural Position

```text
Rust Call Controller
      │
      ├── CallState
      ├── Participant State
      ├── Media State
      ├── Security State
      ├── Reconnect State
      └── Quality State
      │
      ▼
Call Presentation Service
      │
 ┌────┴─────┐
 │          │
Dioxus    Compose
Desktop   Android
```

Platform services:

```text
Desktop:
    windows
    audio-device chooser
    screen capture
    notifications

Android:
    foreground service
    PiP
    audio route
    camera/microphone permissions
    notification actions
```

---

# 3. Core Call UX States

```rust
pub enum CallUiState {
    Incoming,
    OutgoingRinging,
    Negotiating,
    Connecting,
    Active,
    Reconnecting,
    Held,
    Ending,
    Ended,
    Failed,
}
```

These should map directly from Part 29 call semantics.

---

# 4. Call UI Must Not Infer State

Bad:

```text
no media packets for 2 seconds
→ UI assumes call ended
```

Correct:

```text
Rust CallController
→ state update
→ UI reflects state
```

---

# 5. Shared Call Snapshot

```rust
pub struct CallScreenSnapshot {
    pub call_id: CallId,
    pub state: CallUiState,
    pub peer: CallPeerView,
    pub kind: CallKind,
    pub duration: Option<Duration>,
    pub media: CallMediaView,
    pub controls: CallControlAvailability,
    pub quality: CallQualityView,
    pub security: CallSecurityView,
}
```

---

# 6. Incoming Call UX

Must show:

```text
caller identity
avatar
audio/video type
verification state if relevant
Accept
Decline
```

Optional:

```text
Message
Remind me later
```

later.

---

# 7. Caller Identity

Display comes from authenticated contact/account mapping.

Never trust arbitrary call-offer display name.

---

# 8. Unknown Caller

Clearly show:

```text
Unknown caller
```

or:

```text
Message request caller
```

with reduced trust visual language.

---

# 9. Incoming Call Security Warning

If identity changed/unverified:

```text
Identity changed
```

should be visible before accept if security policy requires.

---

# 10. Incoming Video Call

Show:

```text
Video call
```

but camera should not activate before user accepts.

---

# 11. Incoming Audio Call

No camera resources allocated.

---

# 12. Android Incoming Call UX

Potential surfaces:

```text
heads-up notification
full-screen intent where policy/platform allows
lock-screen call UI
Compose full call screen
```

Actual use depends Android rules and user notification permission.

---

# 13. Desktop Incoming Call UX

Possible:

```text
desktop notification
in-app banner
floating incoming-call window
```

if app is visible.

---

# 14. Ringing Priority

A valid incoming call should be visually prominent.

But rate limits prevent abuse.

---

# 15. Accept

User action:

```text
Accept
```

routes to:

```text
Rust CallController.accept(CallId)
```

UI does not directly start media.

---

# 16. Accept Video Call

If camera permission missing:

```text
accept audio first
then request/enable camera
```

or:

```text
request permission before full video accept
```

depending product policy.

Recommendation:

```text
accept call with audio if possible
request camera immediately afterward
```

to reduce connection delay.

---

# 17. Decline

Routes to Rust.

UI transitions to:

```text
Ended / declined
```

---

# 18. Answered Elsewhere

If another device answers:

```text
call UI closes
```

with optional small message:

```text
Answered on another device
```

---

# 19. Outgoing Call UX

States:

```text
Calling…
Ringing…
Connecting…
```

These should remain distinct.

---

# 20. Calling vs Ringing

`Calling…`

means:

```text
offer sent / waiting for remote ringing state
```

`Ringing…`

means:

```text
remote is actively being alerted
```

if protocol exposes it.

---

# 21. Outgoing Cancel

User can cancel before active.

Action:

```text
Cancel
```

---

# 22. Busy

Show:

```text
Busy
```

then end screen.

---

# 23. Declined

Show:

```text
Call declined
```

---

# 24. Timeout

Show:

```text
No answer
```

not generic failure.

---

# 25. Unsupported

Example:

```text
Video is unavailable on this device
```

Offer:

```text
Try audio call
```

if possible.

---

# 26. Negotiating State

Usually brief.

Do not expose:

```text
codec negotiation
key exchange
path probing
```

to ordinary users.

Show:

```text
Connecting…
```

unless diagnostic mode.

---

# 27. Active Call Screen

Core regions:

```text
participant/media area
status/duration
quality/reconnect indicator
control bar
optional secondary controls
```

---

# 28. Audio Call Layout

Main focus:

```text
avatar
name
duration
status
```

Controls:

```text
Mute
Speaker/Audio Route
Video
More
Hang Up
```

---

# 29. Video Call Layout

Main focus:

```text
remote video
local preview
participant overlays
controls
```

---

# 30. Local Preview

Small picture-in-picture-style overlay.

Draggable where useful.

---

# 31. Local Preview Disable

If local camera off:

```text
avatar/placeholder
```

---

# 32. Camera Flip

Android:

```text
front ↔ rear
```

through semantic command.

Desktop:

```text
select camera
```

instead.

---

# 33. Camera Selection

Desktop may expose device chooser.

Android usually offers:

```text
front/rear
```

rather than raw camera list.

---

# 34. Mute

Mute button must clearly reflect state:

```text
Muted
Unmuted
```

---

# 35. Mute Action

Routes to Rust.

Audio backend handles capture behavior.

---

# 36. Video Toggle

```text
Enable video
Disable video
```

without restarting logical call.

---

# 37. Hang Up

Always easy to find.

Use visually distinct destructive control.

---

# 38. Audio Route

Potential routes:

```text
Earpiece
Speaker
Bluetooth
Wired headset
USB
```

depending platform.

---

# 39. Android Audio Route UX

Use:

```text
bottom sheet
```

or compact route selector.

---

# 40. Desktop Audio Device UX

Separate:

```text
Input device
Output device
```

in call settings / device menu.

---

# 41. Audio Route Changes

Should not interrupt call UI.

Display small transient:

```text
Bluetooth headset connected
```

if useful.

---

# 42. Bluetooth Disconnect

If route changes automatically:

```text
Audio switched to speaker
```

small snackbar/banner.

---

# 43. Screen Share

State:

```text
Off
Starting
Active
Stopping
```

---

# 44. Android Screen Share

Requires OS capture consent.

Flow:

```text
tap Share Screen
→ platform permission/capture prompt
→ result
→ Rust call command
```

---

# 45. Desktop Screen Share

May offer:

```text
Entire Screen
Window
Region
```

through native platform capture UI.

---

# 46. Screen Share Indicator

Always clearly visible while active.

---

# 47. Privacy While Sharing

Notification preview policy may switch to:

```text
Generic
```

during active share.

---

# 48. Screen Share Priority

UI should make clear:

```text
screen share is active
```

and provide easy stop control.

---

# 49. Camera + Screen Share

If both active:

```text
screen main
camera thumbnail
```

or vice versa depending use case.

---

# 50. Media Layout Switching

Possible layouts:

```text
Remote Focus
Screen Share Focus
Grid
Speaker Focus
```

---

# 51. 1:1 Video

Default:

```text
remote video fills available space
local preview floating
```

---

# 52. Small Group Call

Grid layout.

---

# 53. Active Speaker

For group calls:

```text
highlight active speaker
```

from Part 26 VAD/audio levels.

---

# 54. Speaker Highlight Accessibility

Do not use color only.

Use:

```text
border
label
screen-reader state
```

---

# 55. Group Participant Tile

Shows:

```text
avatar/video
name
mute
connection state
```

---

# 56. Participant List

Optional panel/sheet.

---

# 57. Group Controls

Potential:

```text
Mute
Video
Screen Share
Participants
More
Hang Up / Leave
```

---

# 58. Host Controls

Future:

```text
remove participant
mute request
lock room
```

only if user has role.

---

# 59. Call Waiting

If another call arrives:

```text
incoming call banner/sheet
```

over active call.

Options:

```text
Decline
End current & answer
Hold current & answer
```

if hold supported.

---

# 60. Hold

Active call may enter:

```text
On hold
```

---

# 61. Local Hold UX

Show:

```text
Call on hold
Resume
```

---

# 62. Remote Hold UX

Show:

```text
Alice put the call on hold
```

if protocol exposes.

---

# 63. Resume

Returns to active call without creating new CallId.

---

# 64. Reconnecting

One of the most important states.

Show:

```text
Reconnecting…
```

without immediately ending call.

---

# 65. Reconnecting Layout

Keep:

```text
participant identity
duration
mute/hangup controls
```

Visible.

Video may freeze/placeholder.

---

# 66. Reconnect Quality

Do not show misleading video as live if frozen.

Overlay:

```text
Connection interrupted
```

---

# 67. Reconnect Recovery

When resumed:

```text
remove overlay
continue same call
```

---

# 68. Reconnect Failure

After backend grace window:

```text
Call ended — connection lost
```

---

# 69. Direct/Relay Changes

Ordinary users should not see:

```text
Switched to relay
```

unless diagnostics.

Call remains visually continuous.

---

# 70. Quality Indicator

Simple semantic classes:

```text
Excellent
Good
Fair
Poor
Reconnecting
```

---

# 71. Quality UI

Use small icon or label.

Only surface prominently when poor.

---

# 72. Advanced Diagnostics

Optional sheet:

```text
codec
resolution
FPS
RTT
loss
jitter
route
hardware/software codec
```

---

# 73. Do Not Overload Normal UI

No packet-loss graph in standard call screen.

---

# 74. Video Quality Adaptation

Resolution/FPS can change silently.

Avoid visual notifications for every adaptation.

---

# 75. Audio Degradation

If backend drops video to preserve audio:

```text
Video paused due to connection
```

small banner.

---

# 76. Audio-Only Fallback

If video becomes unavailable:

```text
call continues as audio
```

---

# 77. Camera Failure

Show:

```text
Camera unavailable
```

with:

```text
Retry
Switch camera
Continue audio
```

where applicable.

---

# 78. Microphone Failure

Show:

```text
Microphone unavailable
```

and keep receive-only call if meaningful.

---

# 79. Permission Denied — Camera

Do not end audio call.

Show:

```text
Camera permission is required for video
Open Settings / Not now
```

---

# 80. Permission Denied — Microphone

Incoming call may become listen-only if supported.

Otherwise show clear limitation.

---

# 81. Android Permission Timing

Request only when user invokes:

```text
video
microphone
screen share
```

---

# 82. Desktop Device Permission

Handle OS permission if platform requires.

---

# 83. Android Foreground Service

Active call persists through:

```text
Activity background
screen off
navigation
```

as platform permits.

---

# 84. Foreground Service Notification

Shows:

```text
ongoing call
mute
hang up
```

where supported.

---

# 85. Compose Activity Recreation

Call survives:

```text
rotation
theme change
window resize
```

---

# 86. Activity Reattachment

New call screen requests:

```text
CallScreenSnapshot
```

from Rust.

---

# 87. PiP

Video call may enter Picture-in-Picture.

---

# 88. PiP Trigger

Could enter when:

```text
user presses Home
```

if active video call.

---

# 89. PiP Content

Minimal:

```text
remote video
call state
```

with system-supported actions where useful.

---

# 90. PiP Controls

Avoid too many.

Possible:

```text
Mute
Hang up
```

---

# 91. Return from PiP

Compose reconstructs full call UI from Rust state.

---

# 92. Audio Call Background

No full-screen UI required.

Ongoing-call notification/compact app bar sufficient.

---

# 93. Desktop Minimized Call

When navigating away:

```text
persistent call bar
```

in main window.

---

# 94. Desktop Floating Call Window

Optional compact floating window.

---

# 95. Floating Window Contents

```text
remote video/avatar
mute
video
return
hang up
```

---

# 96. Desktop Close Call Window

Should not automatically hang up.

Use explicit:

```text
Hang up
```

---

# 97. Desktop Main Window Close

If configured to hide to tray:

```text
call continues
```

---

# 98. Desktop Quit During Call

Confirmation:

```text
End call and quit?
```

---

# 99. Android Back During Call

Back:

```text
leave full call screen
```

call continues.

---

# 100. Android Home

Call continues through foreground service.

---

# 101. App Process Death During Call

If process hosting Rust call dies:

```text
call ends
```

unless separate persistent native/service process architecture preserves it.

UI must not pretend otherwise.

---

# 102. App Restart After Crash

Show:

```text
Call ended unexpectedly
```

in history if known.

---

# 103. Call Duration

Starts at:

```text
connected/active
```

not outgoing ring start.

---

# 104. Duration Timer

UI timer derives from:

```text
connected_at
```

No backend event every second.

---

# 105. Timer Efficiency

One local timer per active call screen is fine.

---

# 106. Incoming Call Timeout

UI closes when backend says:

```text
timeout
```

---

# 107. Missed Call

After timeout:

```text
Missed call
```

notification/history.

---

# 108. Call End Screen

Brief summary:

```text
Call ended
Duration
Call back
```

Optional.

---

# 109. End Reasons

User-facing mappings:

```text
Declined
Busy
No answer
Connection lost
Call ended
Security problem
```

---

# 110. Do Not Show Internal Errors

Avoid:

```text
QUIC stream reset 0x1f
```

in normal UI.

---

# 111. Security Failure

Strong message:

```text
Call ended because identity could not be verified
Review security
```

if applicable.

---

# 112. Device Revoked Mid-Call

Show:

```text
Call ended for security reasons
```

---

# 113. Encryption Indicator

Normal UI can show:

```text
End-to-end encrypted
```

in call info/details.

Not giant lock icon during whole call.

---

# 114. Verified Contact

Optional subtle verification badge near name.

---

# 115. Call Recording

Future optional feature.

If enabled:

```text
visible recording indicator
```

must never be hidden.

---

# 116. Recording Consent

Product/legal policy determines participant notification.

---

# 117. Call Captions

Future.

Could show live captions if local speech recognition exists.

---

# 118. Captions Accessibility

If added:

```text
font size
contrast
speaker labels
```

---

# 119. Call Transcription

Not part of v1.

Do not upload audio to cloud silently.

---

# 120. Accessibility — Call Controls

Every control needs:

```text
label
state
action
```

Examples:

```text
Mute microphone, off
Turn camera off
End call
```

---

# 121. Android TalkBack

Control bar order should be logical and stable.

---

# 122. Desktop Screen Reader

Focused control announces state.

---

# 123. Keyboard Shortcuts Desktop

Potential:

```text
Ctrl/Cmd+Shift+M → Mute
Ctrl/Cmd+Shift+V → Video
Ctrl/Cmd+Shift+S → Screen share
Ctrl/Cmd+Shift+H → Hang up
```

---

# 124. Shortcut Discoverability

Show in:

```text
tooltips
menu
keyboard shortcut reference
```

---

# 125. Android Accessibility Actions

No feature should require:

```text
drag-only
swipe-only
```

---

# 126. Large Touch Targets

Call controls must be easy to hit.

Especially:

```text
Hang Up
Mute
```

---

# 127. Color Independence

Muted/connected/reconnecting states need:

```text
icons
labels
shape
```

not color alone.

---

# 128. Large Font

Name/status/control labels must remain usable.

Video can shrink to preserve controls.

---

# 129. Landscape Android

Controls may move to side/bottom based on available space.

---

# 130. Tablet Call Layout

Potential:

```text
large media region
participant panel
controls
```

---

# 131. Foldable Call Layout

Use hinge-aware placement.

---

# 132. Desktop Wide Layout

Can show:

```text
video
participant list
chat side panel
```

if call-chat integration added.

---

# 133. In-Call Chat

Future optional.

Could expose existing conversation in side panel.

No separate message system.

---

# 134. Call + Conversation

Desktop can open conversation inspector beside call.

Android can offer:

```text
Open chat
```

sheet/destination without ending call.

---

# 135. Media Device Selector Desktop

Can be in:

```text
More
→ Audio & Video Settings
```

---

# 136. Device Selector Fields

```text
Microphone
Speaker
Camera
```

---

# 137. Test Device Preview

Settings can provide local preview outside active call.

---

# 138. Android Device Selection

Camera:

```text
front/rear
```

Audio:

```text
route
```

No unnecessary low-level device list.

---

# 139. Screen Share Source Picker Desktop

Native/system-style picker preferred.

---

# 140. Share Stop

Persistent visible action.

---

# 141. Local Preview Mirroring

Front camera preview may be mirrored locally.

Remote stream should not necessarily be mirrored.

---

# 142. Camera Orientation

Android renderer adapts rotation without call restart.

---

# 143. Video Surface Loss

If Compose destination recreates:

```text
show placeholder briefly
rebind Surface
```

call remains.

---

# 144. Desktop Renderer Rebind

Same principle for window changes.

---

# 145. Remote Video Freeze

Backend may report:

```text
video stalled
```

UI overlays:

```text
Video paused
```

without implying call ended.

---

# 146. Screen Off

Android audio call continues.

Video capture may pause according to policy/platform.

---

# 147. Battery Saver

If backend reduces video:

```text
no alarming message
```

unless user-visible effect significant.

---

# 148. Thermal Event

If video disabled:

```text
Video turned off to cool device
```

small banner.

---

# 149. Media Error Prioritization

Audio is more important.

UI should reflect:

```text
Video unavailable, audio continues
```

rather than presenting full call failure.

---

# 150. Reconnect UX During Network Switch

Examples:

```text
Wi-Fi → cellular
```

UI may briefly show:

```text
Reconnecting…
```

only if media interruption noticeable.

---

# 151. Connectivity Indicator

Avoid showing raw transport names unless diagnostics.

---

# 152. Call Quality Details

Advanced panel can show:

```text
Direct
Relay
LAN
Codec
Bitrate
Packet loss
RTT
```

---

# 153. User-Friendly Quality

Normal UI:

```text
Good connection
Poor connection
```

only if needed.

---

# 154. Call Notifications

Part 31.

When app foreground but not on call screen:

```text
incoming call banner/full-screen flow
```

---

# 155. Ongoing Call Notification

Android required by foreground service.

---

# 156. Notification Actions

```text
Mute
Hang up
```

where safe.

---

# 157. Lock-Screen Call UX

Must not reveal sensitive identity details beyond notification privacy settings.

---

# 158. Unknown Caller Privacy

Could show:

```text
Incoming call
```

with generic identity if user chooses strict preview privacy.

---

# 159. Call History Navigation

After missed/ended call:

```text
tap event
→ call detail / conversation
```

---

# 160. Call Detail

Could show:

```text
type
time
duration
end reason
call back
```

---

# 161. Call History Privacy

Call metadata is sensitive.

Follow local storage/preview policy.

---

# 162. Multi-Device Incoming Call

Several devices may ring.

UI state should handle:

```text
answered elsewhere
declined elsewhere if policy
```

---

# 163. Local Device Priority

Optional future:

```text
prefer active device
```

backend policy.

UI only renders outcome.

---

# 164. Call Transfer Between Devices

Future advanced feature.

Not v1.

Could allow:

```text
Move call to phone
```

with secure handoff.

---

# 165. Call Handoff UX

If added:

```text
choose device
confirm
new device connects
old device exits
```

---

# 166. Group Call Entry

Could start from:

```text
group header
call history
active group call banner
```

---

# 167. Group Call Banner

If group call active:

```text
3 people in call
Join
```

future if architecture supports.

---

# 168. Join Group Call

Rust validates membership/security.

---

# 169. Participant Joining

UI can show subtle:

```text
Alice joined
```

---

# 170. Participant Leaving

Likewise.

---

# 171. Avoid Modal Spam

Do not show dialog for every group participant event.

---

# 172. Raised Hand

Future conference feature.

Not needed initially.

---

# 173. Moderator Controls

Future.

Keep architecture extensible.

---

# 174. Reactions in Call

Future lightweight ephemeral reactions.

Not v1.

---

# 175. Media Control Availability

Rust supplies:

```rust
pub struct CallControlAvailability {
    pub can_mute: bool,
    pub can_video: bool,
    pub can_switch_camera: bool,
    pub can_screen_share: bool,
    pub can_hold: bool,
    pub can_change_audio_route: bool,
}
```

---

# 176. UI Must Respect Capability

Do not show enabled video button if backend says unsupported.

---

# 177. Disabled Control

If useful, show disabled with explanation.

---

# 178. Hidden vs Disabled

Hide:

```text
feature fundamentally unsupported
```

Disable:

```text
temporarily unavailable
```

with reason.

---

# 179. Call Media View

```rust
pub struct CallMediaView {
    pub microphone_muted: bool,
    pub camera_enabled: bool,
    pub screen_share: ScreenShareView,
    pub audio_route: AudioRouteView,
    pub local_renderer: Option<RendererId>,
    pub remote_renderers: Vec<ParticipantRendererView>,
}
```

---

# 180. Call Quality View

```rust
pub struct CallQualityView {
    pub class: CallQualityClass,
    pub video_degraded: bool,
    pub audio_degraded: bool,
}
```

---

# 181. Security View

```rust
pub struct CallSecurityView {
    pub encrypted: bool,
    pub verified_peer: bool,
    pub warning: Option<SecurityWarningView>,
}
```

---

# 182. Call Events

```rust
pub enum CallUiEvent {
    StateChanged(CallUiState),
    MediaChanged(CallMediaView),
    QualityChanged(CallQualityView),
    ParticipantChanged(CallParticipantView),
    SecurityChanged(CallSecurityView),
    Ended(CallEndReasonView),
}
```

---

# 183. Event Granularity

Update only affected UI region.

---

# 184. Android ViewModel

Owns:

```text
temporary sheet state
permission effects
PiP request
selected audio route sheet
local control focus
```

Rust owns call truth.

---

# 185. Dioxus Presenter

Owns:

```text
floating window state
inspector visibility
control overlay visibility
selected participant tile
```

---

# 186. Control Overlay Auto-Hide

Video call can hide controls after inactivity.

---

# 187. Desktop Auto-Hide

Mouse move reveals.

Keyboard focus keeps visible.

---

# 188. Android Auto-Hide

Tap video reveals.

Do not hide while accessibility focus active.

---

# 189. Audio Call Controls

No auto-hide needed.

---

# 190. End Call Confirmation

Usually:

```text
no confirmation
```

because hanging up is expected and reversible only by calling again.

Exception:

```text
host ending call for everyone
```

may need confirmation.

---

# 191. Screen Share Start Confirmation

OS capture prompt provides confirmation.

---

# 192. Device Revocation Security Event

Call ends immediately if Rust policy says so.

UI does not override.

---

# 193. Call Reauthentication

If backend rekeys transparently:

```text
no UI
```

unless security failure.

---

# 194. Accessibility Live Updates

Announce:

```text
Call connected
Reconnecting
Call ended
Muted
```

carefully.

---

# 195. Avoid Excessive Announcements

Do not announce every bitrate change.

---

# 196. Call Status Region

Screen reader live region can report important state transitions.

---

# 197. Haptics Android

Use sparingly:

```text
call accepted
mute toggle
hang up
```

according to platform norms.

---

# 198. Ring/Vibration

Controlled by notification/call policy.

---

# 199. Desktop Sound

Incoming ring through selected/system output.

---

# 200. Ring Output Device

May differ from call output in advanced settings.

---

# 201. Incoming Call Audio Focus Android

Platform call integration handles.

---

# 202. Call Volume

Use system communication volume where appropriate.

---

# 203. App Volume Controls

Do not reinvent OS volume control in v1.

---

# 204. Media Permission Education

If user denies:

```text
explain why feature needs permission
```

without blocking unrelated functionality.

---

# 205. Camera Preview Before Call

Outgoing video call may show local preview optionally.

But do not delay call too much.

---

# 206. Pre-Call Screen

Future optional:

```text
camera preview
mic selector
background
```

for conference-style calling.

Not required for simple 1:1 v1.

---

# 207. Initial v1 Call UX

Prioritize:

```text
1:1 audio
1:1 video
incoming/outgoing
mute
video
route
camera switch
reconnect
PiP/background
desktop call bar/window
```

---

# 208. Defer Initially

```text
large group grid
SFU-specific controls
call recording
live captions
background effects
virtual backgrounds
device handoff
complex moderator controls
```

---

# 209. Testing Matrix

Required:

```text
incoming audio
incoming video
outgoing audio
outgoing video
decline
busy
timeout
active
mute
video toggle
camera switch
audio route
screen share
reconnecting
connection lost
answered elsewhere
permission denied
surface recreation
PiP
desktop window close
```

---

# 210. Android Tests

Verify:

```text
Activity recreation
background/foreground
PiP
foreground service
notification accept
notification decline
camera permission
mic permission
Bluetooth route change
screen-share permission
TalkBack
large font
foldable
```

---

# 211. Desktop Tests

Verify:

```text
floating call window
persistent call bar
window close
system tray
keyboard shortcuts
camera/audio device chooser
screen-share source picker
```

---

# 212. Reconnect Test

Kill Wi-Fi / switch network.

Expected:

```text
same CallId
Reconnecting…
resume
```

---

# 213. Video Failure Test

Force video codec failure.

Expected:

```text
audio continues
video warning/fallback
```

---

# 214. Surface Loss Test

Destroy/recreate renderer.

Call continues.

---

# 215. Permission Denied Test

Video denied.

Audio continues.

---

# 216. Bluetooth Disconnect Test

Route switches.

UI reflects new route.

---

# 217. Answered Elsewhere Test

Phone and desktop ring.

Desktop answers.

Phone exits incoming state.

---

# 218. Hold Test

Current call held/resumed without new call identity.

---

# 219. Call Waiting Test

Second call arrives.

Actions behave deterministically.

---

# 220. Accessibility Test

Full call flow must be possible without visual-only cues.

---

# 221. Rust Presentation API

```rust
pub trait CallPresentation {
    async fn snapshot(
        &self,
        call: CallId,
    ) -> Result<CallScreenSnapshot, UiError>;

    async fn accept(
        &self,
        call: CallId,
    ) -> Result<(), UiError>;

    async fn decline(
        &self,
        call: CallId,
    ) -> Result<(), UiError>;

    async fn hangup(
        &self,
        call: CallId,
    ) -> Result<(), UiError>;

    async fn set_muted(
        &self,
        call: CallId,
        muted: bool,
    ) -> Result<(), UiError>;

    async fn set_video(
        &self,
        call: CallId,
        enabled: bool,
    ) -> Result<(), UiError>;

    async fn set_hold(
        &self,
        call: CallId,
        held: bool,
    ) -> Result<(), UiError>;
}
```

---

# 222. Screen Share API

```rust
pub trait ScreenSharePresentation {
    async fn start(
        &self,
        call: CallId,
        source: ScreenShareSource,
    ) -> Result<(), UiError>;

    async fn stop(
        &self,
        call: CallId,
    ) -> Result<(), UiError>;
}
```

Platform adapter supplies source selection/permission.

---

# 223. Audio Route API

```rust
pub trait AudioRoutePresentation {
    async fn available_routes(
        &self,
        call: CallId,
    ) -> Result<Vec<AudioRouteView>, UiError>;

    async fn select_route(
        &self,
        call: CallId,
        route: AudioRouteId,
    ) -> Result<(), UiError>;
}
```

---

# 224. Camera API

```rust
pub trait CameraPresentation {
    async fn switch_camera(
        &self,
        call: CallId,
    ) -> Result<(), UiError>;
}
```

Desktop may expose explicit device selection.

---

# 225. Android Platform Effects

ViewModel may emit:

```text
RequestCameraPermission
RequestMicrophonePermission
RequestScreenCapture
EnterPiP
OpenBluetoothSettings
```

---

# 226. Desktop Platform Effects

Presenter may emit:

```text
OpenScreenSharePicker
OpenAudioDeviceMenu
CreateCallWindow
RaiseCallWindow
```

---

# 227. No Direct Platform Calls from Rust Domain

Rust can request semantic action.

Platform UI executes it.

---

# 228. Call UI Error Mapping

Examples:

```text
PermissionRequired
MediaUnavailable
PeerUnavailable
SecurityBlocked
CallExpired
RouteUnavailable
```

---

# 229. Error Presentation

Permission:

```text
contextual action
```

Media failure:

```text
degrade if possible
```

Security:

```text
strong warning
```

Expired call:

```text
call ended/missed
```

---

# 230. Metrics

Safe local metrics:

```text
call screen open latency
accept-to-audio time
first-video-frame time
PiP transition
reconnect duration
```

No media content.

---

# 231. Diagnostics Privacy

Do not log:

```text
raw audio
video frames
private keys
```

---

# 232. Screenshot Testing

Required states:

```text
incoming
outgoing ringing
connecting
active audio
active video
muted
video off
screen sharing
reconnecting
poor connection
permission denied
ended
group grid
PiP conceptual
dark mode
large font
RTL
```

---

# 233. UI/UX Part 07 Definition of Done

Part 07 is complete when:

- incoming/outgoing call states map directly from Rust call state
- caller identity is authenticated before normal trusted presentation
- audio/video calls have distinct but consistent layouts
- mute, video, camera switch, audio route, screen share, hold, and hangup are semantic Rust commands
- call remains one logical session through path, route, surface, and codec changes
- reconnecting is visible without prematurely ending call
- video failure can degrade to audio
- Android Activity recreation, PiP, foreground service, and notification actions are defined
- desktop persistent call bar and floating call window behavior are defined
- closing a call window does not automatically hang up
- accessibility, keyboard, TalkBack, large touch targets, large font, and reduced motion are explicit
- permissions are requested contextually and do not destroy call state
- group-call layout can evolve without changing 1:1 semantics
- normal UI hides low-level transport/codec details while diagnostics can expose them
- answer-elsewhere, call waiting, hold, device revocation, connection loss, surface loss, and Bluetooth route changes are tested
- raw media never flows through ordinary Dioxus/Compose state
- the exact Rust presentation APIs and platform effects are defined

---

# 234. Final Architecture

```text
                    RUST CALL CONTROLLER
                              │
                    CallScreenSnapshot
                              │
             ┌────────────────┴────────────────┐
             │                                 │
          Dioxus                            Compose
             │                                 │
 Desktop Call Window                    Android Call Screen
 Persistent Call Bar                    PiP / Foreground Service
             │                                 │
             └──────────────┬──────────────────┘
                            │
                    Semantic Commands
                            │
           Mute / Video / Route / Share / Hold
                            │
                     Rust Media Runtime
```

Platform-owned mechanics:

```text
Desktop:
    windows
    device menus
    screen picker

Android:
    permissions
    foreground service
    notification actions
    PiP
    system capture
```

Call truth remains:

```text
CallId
CallState
MediaState
SecurityState
```

inside Rust.

---

# 235. Final Principle

The call UI should remain calm and stable even when the media system is adapting aggressively underneath it.

The right user-facing model is:

```text
one call
+
clear participant identity
+
few reliable controls
+
graceful reconnect/degradation
+
platform-native background behavior
```

not:

```text
one UI state per codec, transport path, or renderer implementation
```

That separation gives Dioxus desktop and Android Compose a polished realtime communication experience while preserving the transport-independent Rust call architecture.
