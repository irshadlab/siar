# UI/UX Part 17 — Emergency / SOS / Offline Mesh UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 17  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete emergency/SOS, offline mesh, store-carry-forward, responder acknowledgement, emergency priority, location consent, battery-survival, false-trigger prevention, background delivery, accessibility, and Rust presentation architecture across desktop and Android.

---

# 1. Purpose

Emergency communication is fundamentally different from ordinary messaging.

Users may depend on it when:

```text
Internet is unavailable
cellular data is unavailable
infrastructure is damaged
a local Wi-Fi/router still exists
devices can only reach each other through Bluetooth
messages must be physically carried by intermediate devices
battery is critically low
the app is backgrounded
the device is locked
delivery may take minutes or hours
```

The UX must communicate uncertainty clearly without creating false confidence.

The governing principle is:

> **Emergency UX must show what has actually happened, what is still pending, and what delivery paths are available without ever pretending that "sent" means "help is coming."**

---

# 2. Architectural Position

```text
Emergency User Intent
        │
        ▼
Rust Emergency Controller
        │
        ├── priority class
        ├── recipient policy
        ├── routing policy
        ├── DTN/store-carry-forward
        ├── expiry
        ├── acknowledgements
        ├── deduplication
        ├── location payload
        └── security policy
        │
        ▼
Emergency Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

Underlying transport may use:

```text
Internet
LAN
Wi-Fi Direct/Aware
Bluetooth
relay
mesh
DTN
```

The UI remains transport-neutral by default.

---

# 3. Emergency UX Must Not Imply Official Emergency-Service Integration

Unless the product is actually integrated with emergency services:

```text
SOS sent
```

must not be presented as:

```text
Police/ambulance dispatched
```

Hard rule.

---

# 4. Core Emergency Concepts

```text
SOS event
Emergency contact set
Emergency message
Priority class
Delivery route
Acknowledgement
Responder status
Expiry
Location payload
Offline relay
```

---

# 5. Emergency Priority Classes

Recommended:

```rust
pub enum EmergencyPriority {
    Urgent,
    Critical,
    LifeSafety,
}
```

Exact semantics belong to backend policy.

---

# 6. Priority Is Not User Drama

Do not offer a confusing 10-level severity slider.

Prefer:

```text
Urgent
SOS
```

or a small set of clearly explained levels.

---

# 7. Emergency Entry Points

Android:

```text
Chats / main shell → SOS
lock-screen/notification shortcut only if platform policy permits
widget/shortcut optional
```

Desktop:

```text
primary rail / command palette
tray menu
dedicated emergency button
```

---

# 8. Avoid Accidental Activation

SOS requires deliberate activation.

Recommended:

```text
press-and-hold
+
clear confirmation
```

or:

```text
tap
→ confirmation sheet
→ Send SOS
```

---

# 9. False Trigger Protection

Do not use:

```text
single small tap
```

for irreversible high-priority broadcast.

---

# 10. Accessibility Alternative

Hold gestures must have a non-gesture accessible equivalent.

Example:

```text
Open SOS
→ Confirm Send
```

---

# 11. Emergency Activation Flow

Recommended:

```text
Open SOS
→ choose recipients/preset
→ optional message
→ optional location sharing
→ review
→ activate
```

For very fast emergency mode:

```text
preconfigured SOS preset
→ press/hold
→ short cancel countdown
→ send
```

---

# 12. Cancel Countdown

Optional safety mechanism:

```text
Sending SOS in 5…
Cancel
```

Useful for accidental activation.

---

# 13. Countdown Accessibility

Announce remaining time.

Provide large:

```text
Cancel
```

control.

---

# 14. Immediate Mode

If user disables countdown in settings:

```text
explicit warning
```

because accidental activation risk increases.

---

# 15. Emergency Presets

Potential:

```text
Personal SOS
Family SOS
Team Emergency
Disaster Check-In
```

---

# 16. Preset Model

```rust
pub struct EmergencyPresetView {
    pub id: EmergencyPresetId,
    pub name: String,
    pub recipients: EmergencyRecipientPolicyView,
    pub share_location: bool,
    pub priority: EmergencyPriority,
    pub expiry: Duration,
}
```

---

# 17. Trusted Emergency Recipients

User explicitly configures:

```text
trusted contacts
groups
own devices
local emergency group
```

---

# 18. Emergency Contact Configuration

Security-sensitive because these contacts may receive:

```text
priority alerts
location
offline relayed messages
```

---

# 19. Contact Trust

Emergency contact does not automatically mean cryptographically verified.

UI should show verification state during setup.

---

# 20. Emergency Contact Warning

If unverified:

```text
This contact is not verified
```

but user may still choose them.

---

# 21. Recipient Policy Types

```rust
pub enum EmergencyRecipientPolicy {
    SpecificContacts(Vec<AccountId>),
    Group(GroupId),
    AllEmergencyContacts,
    LocalNearbyBroadcast,
}
```

---

# 22. Nearby Emergency Broadcast

Highly sensitive.

Do not broadcast personal identity/location to every nearby device by default.

---

# 23. Local Broadcast Policy

If supported, define:

```text
authenticated known peers only
or
anonymous/distress beacon mode
```

as separate product feature.

---

# 24. Distress Beacon

Optional future mode.

Could advertise:

```text
A nearby user needs help
```

without full identity until responder accepts/authenticates.

---

# 25. Identity Privacy in Beacon Mode

Potential:

```text
ephemeral distress identity
```

until trusted handshake.

---

# 26. SOS Event Model

```rust
pub struct EmergencyEventView {
    pub id: EmergencyEventId,
    pub state: EmergencyEventState,
    pub priority: EmergencyPriority,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub recipients: EmergencyRecipientSummary,
    pub delivery: EmergencyDeliverySummary,
    pub location: Option<EmergencyLocationView>,
}
```

---

# 27. Emergency Event State

```rust
pub enum EmergencyEventState {
    Preparing,
    Active,
    Cancelling,
    Cancelled,
    Expired,
    Resolved,
    Failed,
}
```

---

# 28. Active

Means:

```text
the SOS remains live and eligible for delivery/re-delivery
```

not:

```text
someone has responded
```

---

# 29. Delivery Semantics

Emergency UX should distinguish:

```text
Created
Stored locally
Relaying
Delivered to device
Acknowledged by person
Responder engaged
Resolved
```

---

# 30. Never Collapse These Into One "Sent"

Hard rule.

---

# 31. Emergency Delivery State

```rust
pub enum EmergencyDeliveryState {
    StoredLocal,
    WaitingForRoute,
    Relaying,
    Delivered,
    Acknowledged,
    Failed,
    Expired,
}
```

---

# 32. Stored Local

Meaning:

```text
SOS is durably stored on this device
```

---

# 33. Waiting for Route

Meaning:

```text
no current path to recipient
```

but DTN/mesh may continue trying.

---

# 34. Relaying

Meaning:

```text
message is being carried/forwarded
```

---

# 35. Delivered

Meaning:

```text
recipient device accepted event
```

not necessarily human saw it.

---

# 36. Acknowledged

Meaning:

```text
recipient explicitly acknowledged
```

---

# 37. Responder Engaged

Optional higher-level state:

```text
I'm coming
I called for help
I'm safe / monitoring
```

only if recipient sends explicit response.

---

# 38. Emergency Status Screen

Recommended:

```text
SOS Active
Created 19:42

Recipients
    Alice — Delivered
    Bob — Waiting for route
    Family Group — Relaying

Location
    Shared

Actions
    Update
    Cancel SOS
    Mark Resolved
```

---

# 39. Main Status Language

Use explicit:

```text
Waiting for route
Delivered to device
Acknowledged
```

---

# 40. Avoid Misleading Success Color

Delivered can be positive but not equivalent to rescue.

---

# 41. Emergency Message Content

Default prewritten template:

```text
I need help. This is an emergency.
```

User may add custom text.

---

# 42. Message Length

Bounded tightly for resilient transport.

---

# 43. Minimal Emergency Payload

Should be deliverable even on constrained routes.

Potential:

```text
EventId
sender identity
priority
timestamp
expiry
small text
location summary
auth/signature
```

---

# 44. Attachments

Do not attach large media to core SOS packet.

---

# 45. Emergency Media

Optional secondary message after SOS:

```text
photo
audio note
```

lower priority than core distress payload.

---

# 46. Core SOS First

Hard rule:

```text
small SOS payload sends first
```

then optional rich media.

---

# 47. Location Sharing

Must be explicit and understandable.

Options:

```text
Do not share
Approximate location
Precise location
```

depending platform/backend.

---

# 48. Default Location Policy

User can configure preset.

At send time, show current choice.

---

# 49. Location Permission — Android

Request only when user chooses location sharing.

---

# 50. Permission Denied

SOS still sends without location.

Show:

```text
SOS will be sent without location
```

---

# 51. Location Unavailable

Same.

---

# 52. Last Known Location

If backend uses last known:

```text
Last known location
```

must be labeled clearly.

---

# 53. Location Timestamp

Responder sees:

```text
Location updated 3 min ago
```

---

# 54. Continuous Location Updates

Optional.

Requires explicit choice.

---

# 55. Live Location Duration

Examples:

```text
15 minutes
1 hour
Until SOS resolved
```

---

# 56. Live Location Battery Cost

Explain briefly if meaningful.

---

# 57. Location Precision

Do not display false precision.

---

# 58. Offline Location

GNSS may work without Internet.

Do not equate no Internet with no location.

---

# 59. Location Privacy

Only intended authorized recipients should receive precise location by default.

---

# 60. Nearby Relay Privacy

Intermediate DTN relays should carry encrypted payload without seeing precise content/location where architecture supports.

UI need not explain cryptographic internals but can say:

```text
Relays can forward the SOS without reading its contents
```

if true.

---

# 61. DTN / Store-Carry-Forward

Core emergency feature.

Message may travel:

```text
Phone A
→ nearby Phone B
→ later Phone C
→ recipient
```

---

# 62. User-Facing DTN Language

Use:

```text
Relaying through nearby devices
```

not:

```text
bundle replication hop 4
```

---

# 63. DTN State

```rust
pub struct DtnDeliveryView {
    pub copies_in_flight: u32,
    pub last_forwarded_at: Option<Timestamp>,
    pub route_state: DtnRouteState,
}
```

---

# 64. Do Not Show Exact Relay Count by Default

May leak topology and confuse users.

Normal UI:

```text
Relaying
```

Advanced diagnostics may show more.

---

# 65. Emergency Expiry

Every SOS should have expiry.

---

# 66. Expiry Prevents Forever-Forwarding

After expiry:

```text
stop relaying
```

unless renewed.

---

# 67. Expiry UX

Show:

```text
Active for 58 more minutes
```

or:

```text
Expires at 20:45
```

---

# 68. Renew SOS

If still needed:

```text
Extend
```

creates signed renewal according to backend.

---

# 69. Cancel SOS

Cancellation is itself a high-priority emergency update.

---

# 70. Cancellation Semantics

Cancellation should propagate through same mesh/DTN paths.

---

# 71. Cancellation Is Not Instant Everywhere

UI may show:

```text
Cancellation sent
Some offline devices may receive the cancellation later
```

if true.

---

# 72. Resolved vs Cancelled

Cancelled:

```text
SOS was withdrawn
```

Resolved:

```text
emergency ended successfully/incident closed
```

Different semantics.

---

# 73. False Alarm

Optional reason:

```text
False alarm
```

can propagate as resolution note.

---

# 74. Emergency Acknowledgement

Recipient action:

```text
Acknowledge
```

means:

```text
I saw this
```

not:

```text
I am responding
```

---

# 75. Responder Actions

Potential:

```text
Acknowledge
I'm coming
I contacted emergency services
I'm nearby
Unable to help
```

Keep initial set small.

---

# 76. Responder Status Model

```rust
pub enum EmergencyResponseKind {
    Acknowledged,
    Responding,
    HelpContacted,
    Nearby,
    Unable,
}
```

---

# 77. Response Honesty

Only show status explicitly sent by responder.

---

# 78. Recipient Emergency Screen

Shows:

```text
SOS from Alice
priority
time
location if shared
message
delivery age
verification state
```

Actions:

```text
Acknowledge
Respond
Message
Call
Open Map
```

---

# 79. Verification Warning

If sender unverified:

```text
Sender identity is not verified
```

but do not hide urgent content.

---

# 80. Unknown Emergency Sender

Policy-sensitive.

May show:

```text
Emergency alert from unknown nearby user
```

with caution.

---

# 81. Abuse Resistance

Unknown emergency broadcasts need:

```text
rate limits
authentication policy
reputation/trust gating
```

Backend controls.

---

# 82. User Blocking and Emergencies

Policy decision.

Recommendation:

```text
blocked contacts do not bypass block
```

unless emergency contacts explicitly configured with exception.

---

# 83. Emergency Contact Override

If user explicitly marks someone an emergency contact, policy may permit high-priority delivery despite mute/DND.

---

# 84. Mute vs Emergency

Conversation mute should not silently suppress configured SOS alerts.

---

# 85. Quiet Hours

Emergency channel may bypass app quiet hours if user opted in.

---

# 86. OS DND

Respect platform rules.

Do not promise bypass unless permission/policy exists.

---

# 87. Emergency Notification

Part 13 category:

```text
Emergency
```

high urgency.

---

# 88. Lock-Screen Privacy

Could show:

```text
Emergency alert
```

or sender identity depending user privacy setting.

---

# 89. Android Background

Active outgoing SOS should continue through suitable background/foreground-service architecture where required.

---

# 90. Android Foreground Service

If active location/mesh relaying requires it:

```text
SOS active
```

persistent notification.

---

# 91. Foreground Notification Actions

Potential:

```text
Open
Cancel
Stop Location
```

with high-risk confirmation for cancellation if appropriate.

---

# 92. Android Process Death

Durable SOS state persists in Rust/storage.

On restart:

```text
restore active emergency status
```

---

# 93. Reboot Recovery

If backend/service architecture supports:

```text
resume active SOS
```

after reboot.

If not, UI must not pretend continuity.

---

# 94. Android Battery Saver

Emergency work should receive higher scheduling priority than ordinary sync.

---

# 95. Desktop Emergency Background

Daemon mode:

```text
SOS relaying continues with UI closed
```

---

# 96. Desktop Embedded Mode

If UI/runtime exits:

```text
active SOS may stop
```

unless separate service exists.

Warn before quit:

```text
An SOS is active. Quitting may stop relaying.
```

---

# 97. Desktop Tray

Show:

```text
SOS Active
Open Emergency Status
```

---

# 98. Quit During Active SOS

Strong confirmation.

---

# 99. Emergency Mode UI

Optional simplified survival surface:

```text
SOS status
battery
available routes
nearby peers
location
message/update
```

---

# 100. Survival Mode

When battery low, emergency subsystem may disable:

```text
thumbnails
media indexing
large transfers
presence chatter
animations
```

---

# 101. Survival Mode UI

Show:

```text
Emergency battery mode
Non-essential activity reduced
```

---

# 102. Battery Threshold

Rust/platform decides.

---

# 103. Manual Low-Power Emergency Mode

User may enable:

```text
Preserve Battery
```

---

# 104. Emergency Priority Scheduler

Core traffic priority:

```text
SOS control
SOS acknowledgements
location updates
critical text
```

above:

```text
normal messages
attachments
backup
search indexing
```

---

# 105. Normal UI Effect

Transfers may show:

```text
Paused for emergency traffic
```

if visible.

---

# 106. Radio Strategy

Rust/network layer may use:

```text
BLE discovery
Wi-Fi LAN
Wi-Fi Direct
Internet relay
DTN
```

in parallel/sequentially.

---

# 107. User-Facing Route Summary

Potential:

```text
Internet available
Nearby relay active
Waiting for direct route
```

---

# 108. Default Route UI

Keep simple:

```text
Connected
Relaying
Waiting for route
```

---

# 109. Diagnostics Route View

Part 20 can show exact paths.

---

# 110. Offline Indicator

Emergency screen should not simply say:

```text
Offline
```

if nearby/DTN routes exist.

Use:

```text
No Internet — nearby relay active
```

---

# 111. No Route

If nothing currently reachable:

```text
No route right now
Your SOS is stored and will keep trying
```

if that is true.

---

# 112. Stored-and-Forward Assurance

Only say:

```text
will keep trying
```

if background runtime actually can.

---

# 113. Device Sleep Constraints

On Android, explain limitations only if OS background restrictions are preventing relay.

---

# 114. Background Restriction Warning

```text
Battery restrictions may delay offline relaying
Review Settings
```

only when diagnosed.

---

# 115. Mesh Participation

Users can choose whether their device relays encrypted emergency messages for others.

---

# 116. Relay Participation Setting

Potential:

```text
Allow emergency relay for others
```

---

# 117. Relay Privacy Explanation

If true:

```text
Your device can forward encrypted emergency messages without reading them.
```

---

# 118. Relay Resource Limit

User may configure:

```text
Only while charging
Battery above 20%
Wi-Fi only
Always for emergencies
```

---

# 119. Default Recommendation

Allow emergency relay with strict battery/data bounds if user opts in.

---

# 120. Data Usage

Emergency packets should be small.

---

# 121. Roaming/Metered Network

Emergency mode may override ordinary Wi-Fi-only policy if user enabled emergency override.

---

# 122. Emergency Data Override

Explicit setting.

---

# 123. Message Duplication

SOS may arrive over multiple paths.

Deduplicate by:

```text
EmergencyEventId
```

---

# 124. Duplicate Notification

One logical SOS alert.

---

# 125. Updated SOS

Updates use same event identity plus revision.

---

# 126. Emergency Revision

```rust
pub struct EmergencyRevision(pub u64);
```

---

# 127. Update Types

Potential:

```text
location update
message update
priority escalation
cancellation
resolution
```

---

# 128. Event Ordering

Rust resolves revision order.

UI does not sort by arrival time.

---

# 129. Stale Update

Ignored.

---

# 130. Cancellation Race

If acknowledgement and cancellation cross:

```text
UI reflects current Rust event state
```

---

# 131. Expiry Race

Late relay after expiry should be rejected/marked expired by backend.

---

# 132. Emergency Group Broadcast

Group SOS can target:

```text
family
team
school staff
local response group
```

---

# 133. Group Acknowledgements

Aggregate:

```text
3 acknowledged
1 responding
```

---

# 134. Large Group

Do not list hundreds of receipt states inline.

Use summary + detail screen.

---

# 135. Emergency Roles

Future managed groups may have:

```text
Coordinator
Responder
Member
```

---

# 136. Role Authority

Rust capability-driven.

---

# 137. Coordinator Actions

Potential:

```text
mark incident resolved
broadcast update
assign responder
```

future.

---

# 138. Personal SOS v1

Keep simpler than incident-management platform.

---

# 139. Emergency History

Durable list:

```text
Active
Recent
Resolved
Cancelled
Expired
```

---

# 140. Emergency History Privacy

Sensitive.

Can include:

```text
location
recipients
responses
```

Protect with app lock/security policy.

---

# 141. Emergency Event Detail

Show:

```text
timeline
delivery states
acknowledgements
location updates
resolution
```

---

# 142. Timeline Example

```text
19:42 SOS created
19:42 Stored locally
19:43 Relaying nearby
19:46 Delivered to Alice
19:47 Alice acknowledged
19:49 Alice: Responding
20:02 SOS resolved
```

---

# 143. Audit Reliability

These timestamps come from Rust durable event log.

---

# 144. Emergency Search

Part 11 may index event metadata if user permits.

Do not index precise location by default.

---

# 145. Emergency Export

Part 16 may export incident history on explicit action.

---

# 146. Emergency Backup

Part 33 may include resolved event history.

Active ephemeral relay state should not restore as active emergency after arbitrary backup restore.

---

# 147. Critical Restore Rule

Restoring an old backup must never reactivate an old SOS.

---

# 148. Migration

Active SOS during device migration is complex.

Recommendation:

```text
do not migrate active emergency session
```

keep source device active until resolved.

---

# 149. App Update

Active SOS should survive compatible app update if runtime/service supports.

---

# 150. Security

Emergency payloads remain authenticated.

---

# 151. E2EE

Recipient-targeted SOS should use end-to-end encryption.

---

# 152. Relay Visibility

Intermediate relay ideally sees only bounded routing metadata.

---

# 153. Metadata Minimization

Avoid exposing:

```text
full recipient list
precise location
message body
```

to relay nodes.

---

# 154. Unknown Relay

Relay participation must not grant account/contact trust.

---

# 155. Abuse Reporting

Emergency alerts from unknown sources can be:

```text
Block
Report
```

if system supports.

---

# 156. Rate Limits

Prevent SOS spam.

But avoid rate limits that prevent genuine repeated escalation.

Backend policy needs care.

---

# 157. Sender Cancellation Authentication

Only authorized sender/account can cancel their event, except managed coordinator policy.

---

# 158. Responder Acknowledgement Authentication

Signed/authenticated.

---

# 159. Location Update Authentication

Same event identity/revision.

---

# 160. Emergency Presentation Snapshot

```rust
pub struct EmergencyScreenSnapshot {
    pub active: Option<EmergencyEventView>,
    pub presets: Vec<EmergencyPresetView>,
    pub connectivity: EmergencyConnectivityView,
    pub relay_policy: EmergencyRelayPolicyView,
    pub battery_mode: EmergencyBatteryModeView,
}
```

---

# 161. Connectivity View

```rust
pub struct EmergencyConnectivityView {
    pub internet: bool,
    pub nearby_route: bool,
    pub relay_active: bool,
    pub store_carry_forward_available: bool,
}
```

---

# 162. User-Facing Connectivity Mapping

Examples:

```text
Connected
No Internet — nearby relay available
Stored — waiting for route
```

---

# 163. Emergency Delivery Summary

```rust
pub struct EmergencyDeliverySummary {
    pub stored_local: bool,
    pub delivered_recipients: u32,
    pub acknowledged_recipients: u32,
    pub responding_recipients: u32,
    pub waiting_recipients: u32,
}
```

---

# 164. Emergency Presentation API

```rust
pub trait EmergencyPresentation {
    async fn snapshot(
        &self,
    ) -> Result<EmergencyScreenSnapshot, UiError>;

    async fn prepare(
        &self,
        preset: EmergencyPresetId,
    ) -> Result<EmergencyDraftView, UiError>;

    async fn activate(
        &self,
        command: ActivateEmergencyCommand,
    ) -> Result<EmergencyEventView, UiError>;

    async fn update(
        &self,
        command: UpdateEmergencyCommand,
    ) -> Result<EmergencyEventView, UiError>;

    async fn cancel(
        &self,
        event: EmergencyEventId,
    ) -> Result<EmergencyEventView, UiError>;

    async fn resolve(
        &self,
        event: EmergencyEventId,
    ) -> Result<EmergencyEventView, UiError>;
}
```

---

# 165. Responder Presentation API

```rust
pub trait EmergencyResponderPresentation {
    async fn event(
        &self,
        id: EmergencyEventId,
    ) -> Result<EmergencyIncomingView, UiError>;

    async fn respond(
        &self,
        id: EmergencyEventId,
        response: EmergencyResponseKind,
    ) -> Result<(), UiError>;
}
```

---

# 166. Relay Presentation API

```rust
pub trait EmergencyRelayPresentation {
    async fn policy(
        &self,
    ) -> Result<EmergencyRelayPolicyView, UiError>;

    async fn update_policy(
        &self,
        update: EmergencyRelayPolicyUpdate,
    ) -> Result<(), UiError>;
}
```

---

# 167. Location Presentation Boundary

Rust requests semantic location:

```rust
pub enum EmergencyLocationRequest {
    Approximate,
    Precise,
    Continuous { duration: Duration },
}
```

Kotlin/platform obtains permission/location result.

---

# 168. Android Platform Effects

```text
RequestLocationPermission
StartEmergencyForegroundService
OpenMap
OpenBatterySettings
EnableNearby
```

---

# 169. Desktop Platform Effects

```text
RaiseEmergencyWindow
ShowNativeEmergencyNotification
OpenMap
ConfirmQuitDuringSOS
```

---

# 170. Emergency Events

```rust
pub enum EmergencyUiEvent {
    EventChanged(EmergencyEventView),
    DeliveryChanged(EmergencyDeliverySummary),
    ResponseReceived(EmergencyResponseView),
    ConnectivityChanged(EmergencyConnectivityView),
    BatteryModeChanged(EmergencyBatteryModeView),
}
```

---

# 171. Android ViewModel

Owns:

```text
confirmation sheet
countdown
permission effects
map/navigation effects
screen presentation
```

Rust owns:

```text
event identity
delivery
expiry
routing
acknowledgements
```

---

# 172. Dioxus Presenter

Owns:

```text
emergency panel/window
confirmation dialog
timeline presentation
diagnostic expansion
```

---

# 173. Countdown Ownership

UI can render countdown.

Rust owns activation deadline/session so process/lifecycle changes cannot bypass safety semantics.

---

# 174. Process Death During Countdown

On restart:

```text
Rust tells whether event activated/cancelled/expired
```

Do not recreate timer from guessed local state.

---

# 175. Process Death During Active SOS

Reload active event.

---

# 176. Android Lock Screen

Incoming emergency alert should be actionable according to notification privacy and platform rules.

---

# 177. Outgoing SOS Lock Screen

Persistent status notification can show:

```text
SOS active
```

without exposing details.

---

# 178. Accessibility — SOS Activation

Screen reader must clearly announce:

```text
Send SOS to 3 emergency contacts
```

before final activation.

---

# 179. Accessibility — Status

Examples:

```text
SOS active. Delivered to 2 of 3 contacts. One acknowledgement.
```

---

# 180. Location Accessibility

```text
Precise location shared
```

or:

```text
Location not shared
```

---

# 181. Response Accessibility

```text
Alice acknowledged
Bob is responding
```

---

# 182. Color Independence

Emergency status uses:

```text
text
icons
shape
```

not red alone.

---

# 183. Large Font

Critical controls remain visible.

---

# 184. RTL

Emergency content/localized labels mirror correctly.

Coordinates/technical codes preserve canonical formatting.

---

# 185. Reduced Motion

No pulsing/strobing dependency.

---

# 186. Haptics Android

Use meaningful pattern for:

```text
SOS activation
incoming SOS
acknowledgement
```

respecting accessibility/system policy.

---

# 187. Sound

Distinct emergency tone optional.

Must be user-controlled/platform-compliant.

---

# 188. Visual Flash

Avoid unsafe flashing.

---

# 189. High Contrast

Emergency controls should remain legible outdoors/low-light.

---

# 190. One-Handed Android UX

Critical buttons reachable.

---

# 191. Glove/Stress UX

Large touch targets.

Minimal text entry required.

---

# 192. Offline UX Clarity

Examples:

```text
No Internet
Nearby relay active
```

or:

```text
No route right now
SOS stored safely on this device
```

---

# 193. Never Show False "Delivered"

Until recipient device acknowledgement of delivery semantics exists.

---

# 194. Never Show "Help Is Coming"

Unless explicit responder message says so.

---

# 195. Responder Confirmation

If responder selects:

```text
I'm coming
```

then UI may display exactly that.

---

# 196. Expired Unacknowledged SOS

History:

```text
Expired — no acknowledgement received
```

---

# 197. Expired with Delivery

Could show:

```text
Expired — delivered to 2 contacts, no acknowledgement
```

---

# 198. Resolved Event

Stop relaying.

---

# 199. Resolution Note

Optional:

```text
I'm safe now
```

---

# 200. Emergency Settings

Recommended:

```text
Emergency Contacts
SOS Preset
Location Sharing
Cancel Countdown
Emergency Notifications
Offline Relay Participation
Battery/Data Policy
```

---

# 201. Test SOS

Important feature:

```text
Test Emergency Setup
```

---

# 202. Test Mode

Must be clearly marked:

```text
TEST
```

and must not accidentally trigger real emergency behavior.

---

# 203. Test Mode Purpose

Validate:

```text
contacts
notifications
local relay
permissions
location
```

---

# 204. Test Event Identity

Separate event type.

Recipients see:

```text
Test SOS
```

---

# 205. Recovery From Test

No real emergency history confusion.

---

# 206. Onboarding

Do not force emergency setup during first-run.

Offer later.

---

# 207. Emergency Readiness Card

Settings/security can show:

```text
2 emergency contacts configured
Location permission ready
Offline relay enabled
```

---

# 208. Permission Readiness

Android can detect:

```text
notifications
location
nearby
battery restrictions
```

without asking until needed where possible.

---

# 209. Emergency Diagnostics

Useful:

```text
Last route available
Nearby relay status
Background restrictions
Notification permission
Location permission
Battery policy
```

---

# 210. No Secret Diagnostics

Do not expose private keys or full location logs unnecessarily.

---

# 211. Telemetry

Emergency events are highly sensitive.

Default analytics should not collect:

```text
SOS content
location
recipients
response text
```

---

# 212. Safe Metrics

If product absolutely needs reliability metrics:

```text
delivery latency bucket
route type bucket
success/failure class
battery mode
```

with strong privacy controls and no identities/location.

---

# 213. Crash Reports

Redact emergency content/location.

---

# 214. Emergency History Retention

User-controlled or security policy.

---

# 215. Delete Emergency History

Separate from active event cancellation.

---

# 216. Active Event Cannot Be Deleted

Resolve/cancel first.

---

# 217. Export Incident

Explicit action only.

---

# 218. Multi-Device Outgoing SOS

If user triggers SOS on phone:

```text
other trusted devices may show active SOS
```

but should not create duplicate event.

---

# 219. Emergency Event Ownership

Account-level event with originating device metadata.

---

# 220. Other Device Update

Desktop can show:

```text
SOS active from your phone
```

---

# 221. Cancel From Another Trusted Device

Optional capability.

High-risk, Rust-authorized.

---

# 222. Multi-Device Location

Use originating/current authorized source according to policy.

Do not combine silently.

---

# 223. Device Revocation During SOS

If origin device revoked:

```text
security policy decides whether event remains valid
```

UI reflects Rust result.

---

# 224. Emergency Contact Removed Mid-SOS

Existing active recipient set may remain frozen for event consistency.

Future updates use event policy.

---

# 225. Group Membership Change Mid-SOS

Same: Rust owns recipient snapshot/change semantics.

---

# 226. Large-Scale Disaster Mode

Future architecture can support:

```text
community relays
incident channels
resource requests
safe check-ins
```

but keep personal SOS v1 focused.

---

# 227. Initial Production Scope

Ship:

```text
personal SOS
preconfigured emergency contacts
optional precise/approximate location
small text payload
offline store-carry-forward
nearby relay
explicit delivery/acknowledgement states
cancellation
resolution
expiry
Android foreground/background handling
desktop daemon/tray behavior
battery-preservation mode
test SOS
```

Defer:

```text
public anonymous distress network
official emergency-service dispatch integration
large incident command system
medical profile broadcasting
crowdsourced disaster mapping
```

unless explicitly designed, reviewed, and supported.

---

# 228. Testing Matrix

Required:

```text
normal online SOS
no Internet + LAN
Bluetooth-only
no route
DTN relay
delivery
acknowledgement
responder state
cancel
resolve
expiry
location denied
location stale
battery low
process death
multi-device
```

---

# 229. Android Tests

Verify:

```text
foreground service
lock-screen notification
location permission
nearby permission
battery saver
background restriction
process death
reboot recovery if supported
TalkBack
large font
```

---

# 230. Desktop Tests

Verify:

```text
daemon active
UI closed
tray status
quit warning
native alert
offline relay
keyboard accessibility
```

---

# 231. DTN Tests

```text
message stored
peer appears later
relay forwards
duplicate path
expiry
cancellation propagation
```

---

# 232. False Trigger Tests

Verify:

```text
hold/cancel
countdown
accessibility alternative
process death during countdown
```

---

# 233. Security Tests

```text
forged SOS
forged acknowledgement
stale revision
unauthorized cancel
replay
unknown relay
```

---

# 234. Privacy Tests

```text
location omitted
approximate only
precise only to intended recipients
relay cannot access plaintext if E2EE
```

---

# 235. Multi-Device Tests

One SOS event appears on all own devices without duplicate notifications/event IDs.

---

# 236. Scale Tests

Many relay copies remain bounded by Rust quotas.

UI remains one logical event.

---

# 237. Accessibility Tests

Complete:

```text
activate
cancel
view delivery
acknowledge incoming SOS
resolve
```

without color, precise gestures, or visual QR-like interaction.

---

# 238. Definition of Done

UI/UX Part 17 is complete when:

- emergency/SOS is clearly separated from ordinary messaging
- the UI never implies official emergency-service dispatch unless truly integrated
- SOS activation has deliberate false-trigger protection and accessible alternatives
- trusted emergency recipients and verification state are visible during configuration
- location sharing is explicit, optional, precision-aware, and never required for SOS delivery
- core SOS payload remains small and higher priority than media
- Stored Local, Waiting for Route, Relaying, Delivered, Acknowledged, Responding, Resolved, Cancelled, and Expired are distinct
- "sent" is never used as misleading proof that help is coming
- offline LAN/Bluetooth/mesh/DTN delivery is represented without transport jargon
- expiry and cancellation propagate through the same resilient system
- duplicate delivery paths collapse into one EmergencyEventId
- responder acknowledgements/status are explicit and authenticated
- Android foreground-service, lock-screen, permission, battery, and process-death behavior are defined
- desktop daemon/tray/quit-during-SOS behavior is defined
- emergency battery mode can reduce non-essential system work
- relay participation has clear privacy/resource controls
- emergency history, test mode, multi-device behavior, accessibility, privacy, and diagnostics are explicit
- Rust emergency, responder, relay, location-request, and UI-event contracts are specified
- online/offline/DTN/security/false-trigger/privacy/process-death tests are included

---

# 239. Final Architecture

```text
                    EMERGENCY USER INTENT
                              │
                              ▼
                    Rust Emergency Core
                              │
          ┌───────────────────┼───────────────────┐
          │                   │                   │
       Priority             Routing           Security
          │                   │                   │
       Expiry           Internet/LAN/DTN       E2EE/Auth
          │                   │                   │
          └───────────────────┼───────────────────┘
                              │
                  Emergency Presentation
                     ┌────────┴────────┐
                     │                 │
                  Dioxus            Compose
                     │                 │
             Desktop SOS UX     Android SOS UX
```

Delivery truth:

```text
Stored
→ Waiting for Route
→ Relaying
→ Delivered
→ Acknowledged
→ Responding
→ Resolved
```

not:

```text
Send button tapped
→ "Help is coming"
```

---

# 240. Final Principle

Emergency UX must optimize for truth under uncertainty.

The correct model is:

```text
small authenticated SOS
+
explicit recipients
+
optional privacy-conscious location
+
offline store-carry-forward
+
clear delivery/acknowledgement semantics
+
battery-aware persistence
```

not:

```text
a red button that displays "sent" and leaves the user guessing what actually happened
```

This gives Dioxus desktop and Android Compose a resilient emergency experience that remains understandable even when the network is fragmented, delayed, or entirely local.
