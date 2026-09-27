# UI/UX Part 12 — Nearby, QR/NFC Pairing & Device Linking UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 12  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete nearby-discovery, QR, NFC, SAS, contact-pairing, own-device linking, offline bootstrap, permission, trust, replay/expiry, mismatch, accessibility, privacy, and Rust presentation architecture across desktop and Android.

---

# 1. Purpose

Nearby/bootstrap flows are where an initially unknown physical device becomes one of several very different things:

```text
a discovered nearby candidate
a contact
a cryptographically verified contact
one of my authorized devices
a temporary pairing session
```

These states must never be blurred.

The governing principle is:

> **Proximity proves only that something is nearby. Authentication and authorization must still be explicit.**

---

# 2. Architectural Position

```text
Platform Discovery
 BLE / Wi-Fi / NFC / Camera
          │
          ▼
Rust Proximity Abstraction
          │
          ├── observations
          ├── rotating discovery identifiers
          ├── authenticated bootstrap
          ├── invite validation
          ├── SAS transcript
          └── device authorization
          │
          ▼
Bootstrap Presentation Service
          │
   ┌──────┴──────┐
   │             │
Dioxus        Compose
Desktop       Android
```

---

# 3. Critical Trust Distinctions

```text
Nearby
≠
Known contact
≠
Accepted contact
≠
Verified contact
≠
Authorized own device
```

The UI must preserve these distinctions in labels, colors, actions, confirmation language, and navigation.

---

# 4. Main Entry Points

Recommended:

```text
Contacts → Add Contact
Contacts → Nearby
Contacts → Scan QR
Devices → Link New Device
Devices → Show Link QR
```

Optional Android shortcut:

```text
Tap NFC
```

---

# 5. Do Not Use One Generic "Pair" Button

Bad:

```text
Pair device
```

for both contacts and account-device authorization.

Use explicit language:

```text
Add Contact
Verify Contact
Link My Device
```

---

# 6. Nearby Discovery Concepts

Rust exposes nearby candidates as observations.

```rust
pub struct NearbyCandidateView {
    pub candidate_id: NearbyCandidateId,
    pub display_hint: Option<String>,
    pub proximity: ProximityHint,
    pub state: NearbyCandidateState,
    pub method: NearbyMethodSummary,
}
```

---

# 7. Candidate Identity

`NearbyCandidateId` is temporary.

It is not:

```text
AccountId
DeviceId
```

until authenticated.

---

# 8. Nearby Candidate States

```rust
pub enum NearbyCandidateState {
    Discovered,
    Connecting,
    Authenticating,
    AwaitingConfirmation,
    Authenticated,
    Failed,
    Expired,
}
```

---

# 9. Nearby Methods

User-facing:

```text
Nearby
```

Normal UI should not care whether candidate came from:

```text
BLE
Wi-Fi Aware
Wi-Fi Direct
LAN
Bluetooth Classic
```

---

# 10. Diagnostics

Advanced diagnostics may expose underlying method.

---

# 11. Proximity Hint

Possible user-facing classes:

```rust
pub enum ProximityHint {
    VeryNear,
    Near,
    Nearby,
    Unknown,
}
```

Avoid displaying false precision such as:

```text
1.7 meters
```

unless hardware truly supports reliable distance.

---

# 12. RSSI

Raw RSSI belongs only in diagnostics.

---

# 13. Nearby Discovery Screen — Android

Recommended:

```text
TopAppBar
    Nearby

Discovery status
    Searching nearby…

LazyColumn
    Candidate
    Candidate
    Candidate

Actions
    Scan QR
    Enter Code
```

---

# 14. Nearby Discovery Screen — Desktop

Recommended:

```text
Nearby Devices
Search status
Candidate list

Alternative:
    Show My QR
    Scan QR
    Enter Code
```

---

# 15. Discovery Start

User explicitly enters Nearby or taps:

```text
Find Nearby
```

Then Rust/platform starts bounded discovery.

---

# 16. No Permanent Discovery

Do not continuously advertise/scan indefinitely unless user enables a deliberate feature.

---

# 17. Discovery Timeout

Example:

```text
30–120 seconds
```

then:

```text
No nearby devices found
Try again
```

Tune by platform/radio behavior.

---

# 18. Discovery Privacy

Use rotating/ephemeral identifiers.

UI must not expose stable account ID over unauthenticated nearby advertising.

---

# 19. Unknown Candidate

Show:

```text
Nearby device
Unverified
```

not a trusted profile.

---

# 20. Display Hint

A peer may advertise a human-friendly hint only if privacy policy permits.

Treat it as untrusted until authentication.

---

# 21. Selecting Nearby Candidate

Flow:

```text
Tap/click candidate
→ connect
→ authenticate bootstrap
→ compare/confirm identity
→ choose resulting relationship
```

---

# 22. Candidate Connection

Show:

```text
Connecting…
```

---

# 23. Authentication

Normal user-facing text:

```text
Verifying device…
```

Do not expose transcript internals.

---

# 24. Confirmation

Once authenticated, show:

```text
identity preview
name
avatar
verification method
SAS/QR confirmation if required
```

---

# 25. Nearby Failure

Semantic reasons:

```text
Device moved out of range
Connection failed
Session expired
Identity could not be verified
Unsupported version
```

---

# 26. Retry

Creates a new session where required.

Never reuse expired nonce/session blindly.

---

# 27. QR Bootstrap Types

Use distinct payload types:

```rust
pub enum BootstrapQrKind {
    ContactInvite,
    ContactVerification,
    DeviceLink,
    GroupInvite,
}
```

---

# 28. QR Visual Differentiation

Screen title must state:

```text
My Contact QR
Verify Identity
Link New Device
Group Invite
```

Do not rely on QR appearance alone.

---

# 29. Contact Invite QR

Purpose:

```text
share contact identity/invite
```

May allow recipient to:

```text
add/accept contact
```

but does not automatically imply completed verification unless protocol flow explicitly authenticates it.

---

# 30. Contact Verification QR

Purpose:

```text
verify an already-known contact
```

Both devices participate in verification transcript.

---

# 31. Device Link QR

Purpose:

```text
authorize another device under my account
```

This is the highest-risk QR flow.

---

# 32. Device-Link Warning

Before showing/accepting:

```text
A linked device can access your account and may receive your messages.
Only link a device you control.
```

---

# 33. Group Invite QR

Routes to Part 09.

Not interpreted as contact/device bootstrap.

---

# 34. QR Parser

Rust owns payload parsing and cryptographic validation.

UI never decides QR type from loose text heuristics.

---

# 35. Unknown QR

Show:

```text
This code is not supported
```

Never open arbitrary URL automatically from scanner unless explicit product behavior.

---

# 36. Malformed QR

Show safe error.

No crash.

---

# 37. Expired QR

```text
This code has expired
Ask for a new code
```

---

# 38. Revoked QR

```text
This code is no longer valid
```

---

# 39. Already Used QR

For single-use invite:

```text
This code has already been used
```

---

# 40. Self QR

Scanning own contact identity:

```text
This is your identity
```

Do not create self-contact.

---

# 41. Contact QR Screen

Recommended contents:

```text
QR
display name
short fingerprint
expiry if applicable
share/copy code
```

---

# 42. Device Link QR Screen

Recommended contents:

```text
Link another device
QR
expires in…
Cancel
```

Avoid contact-style avatar/social language.

---

# 43. QR Expiry Countdown

Can show minute-level countdown.

Do not run expensive per-row timers.

---

# 44. Screenshot Sharing

Assume a QR can be photographed/copied.

Security cannot depend on physical display secrecy alone.

---

# 45. NFC Bootstrap

Android-specific high-quality shortcut.

Flow:

```text
Tap phones
→ Android NFC
→ Kotlin obtains payload
→ Rust validates
→ confirmation
```

---

# 46. NFC Is Transport, Not Trust

NFC proximity alone does not authorize account/device.

---

# 47. NFC Failure

Offer fallback:

```text
Scan QR instead
```

---

# 48. NFC Unsupported

Hide or disable NFC action.

---

# 49. SAS Verification

Short Authentication String allows users to compare:

```text
numbers
words
symbols
```

---

# 50. Recommended SAS UX

Show a short readable code:

```text
482 119
```

or word sequence.

Buttons:

```text
Codes match
Codes do not match
```

---

# 51. Do Not Use "Confirm" Alone

Explicit wording reduces mistakes.

---

# 52. SAS Accessibility

Code should be:

```text
large
high contrast
screen-reader friendly
grouped
```

---

# 53. SAS Canonical Reading

Screen reader can announce groups distinctly.

Example:

```text
482, 119
```

---

# 54. SAS Mismatch

Strong result:

```text
Codes do not match
Do not continue
```

Actions:

```text
Cancel
Try again
Block
```

depending context.

---

# 55. Mismatch Never Marks Verified

Hard rule.

---

# 56. Interrupted Verification

Remains:

```text
Unverified
```

---

# 57. Verification Success

Only after Rust confirms transcript:

```text
Identity verified
```

---

# 58. Verification Method History

Contact security details can record:

```text
Verified by QR
Verified by code comparison
```

with timestamp if useful.

---

# 59. Contact Pairing Flow

Recommended:

```text
Add Contact
   ↓
QR / Nearby / Code
   ↓
Authenticated Identity Preview
   ↓
Accept Contact
   ↓
Optional/Integrated Verification
   ↓
Contact Ready
```

---

# 60. Acceptance vs Verification

UI can show:

```text
Contact added
Identity not yet verified
```

if verification not completed.

---

# 61. Verify Later

Contact profile can expose:

```text
Verify Identity
```

---

# 62. Manual Code Entry

Fallback for:

```text
no camera
desktop
accessibility
remote sharing
```

---

# 63. Manual Code Input

Use grouped fields or one pasteable field.

---

# 64. Paste Support

Allow copy/paste invite code.

---

# 65. Code Validation

Rust authoritative.

---

# 66. Typo UX

Show:

```text
Invalid code
```

without leaking detailed parsing internals.

---

# 67. Own-Device Linking Flow

Recommended:

```text
Existing trusted device:
    Devices
    → Link New Device
    → Show QR/code

New device:
    Set up / Link Existing Account
    → Scan QR/code
    → Authenticate
    → Existing device approves
    → Optional SAS confirmation
    → New Device Authorized
```

---

# 68. Device-Link Roles

Existing device:

```text
authorizer
```

New device:

```text
candidate
```

---

# 69. New Device Identity

Normally gets fresh:

```text
DeviceId
device keys
```

It does not clone old device identity.

---

# 70. Device Link Snapshot

```rust
pub struct DeviceLinkSessionView {
    pub session: DeviceLinkSessionId,
    pub state: DeviceLinkState,
    pub candidate_name: Option<String>,
    pub expires_at: Timestamp,
    pub sas: Option<SasView>,
}
```

---

# 71. Device Link States

```rust
pub enum DeviceLinkState {
    Created,
    Scanned,
    Authenticating,
    AwaitingApproval,
    AwaitingSas,
    Authorizing,
    Complete,
    Failed,
    Expired,
    Cancelled,
}
```

---

# 72. Explicit Approval

Existing device must show:

```text
Link "Pixel 10" to your account?
```

with consequences.

---

# 73. Candidate Device Name

Treat proposed name as untrusted until linked.

---

# 74. Approval Actions

```text
Link Device
Cancel
```

---

# 75. High-Risk Confirmation

Could require:

```text
biometric/device credential
```

on Android in high-security mode.

Desktop may request OS authentication where available.

---

# 76. Device Link Completion

Show:

```text
Device linked successfully
```

Then route to:

```text
Devices
```

---

# 77. New Device Sync

After authorization:

```text
identity metadata
contacts
messages/history according to policy
settings
```

may sync.

UI should show separate:

```text
Setting up your device…
```

not keep "linking" indefinitely.

---

# 78. Device Setup Progress

Possible semantic phases:

```text
Authorizing
Syncing account
Syncing conversations
Rebuilding search
Ready
```

---

# 79. Search Rebuild

Part 11 derived local index.

Do not block device-ready state on full reindex unless required.

---

# 80. Device Link Cancellation

Either side may cancel.

Session becomes unusable.

---

# 81. Device Link Expiry

Short-lived.

New code required.

---

# 82. Replay Protection

If old code reused:

```text
This link code is no longer valid
```

No details necessary.

---

# 83. Existing Device Lost During Link

New candidate does not become authorized without confirmed backend/security completion.

---

# 84. New Device Crashes During Link

Session may be resumed only if secure protocol permits.

Otherwise restart.

---

# 85. Duplicate Device Link

If same device already linked:

```text
This device is already linked
```

---

# 86. Revoked Device Re-Link

Should require fresh authorization.

---

# 87. Offline Device Linking

Ideal local-first flow:

```text
QR
+
local Wi-Fi/QUIC/Bluetooth
```

can authorize without Internet if protocol/security supports.

---

# 88. Offline Contact Verification

Likewise possible through:

```text
QR/SAS/local transport
```

---

# 89. Offline UX

Do not display:

```text
No Internet
```

as failure if local pairing can continue.

---

# 90. Connectivity Requirement

If a specific step needs Internet:

```text
Internet is required for this step
```

only then.

---

# 91. Android Camera Flow

Compose:

```text
Scan QR
→ request camera permission contextually
→ scanner
→ payload
→ Rust
```

---

# 92. Camera Permission Denied

Offer:

```text
Enter code manually
Open Settings
```

---

# 93. Android Nearby Permissions

Kotlin handles version-specific permissions for:

```text
Bluetooth
Nearby Wi-Fi
location-related legacy requirements
```

Rust sees normalized capability.

---

# 94. Permission Explanation

Use accurate reason:

```text
Nearby access lets this app discover devices close to you.
```

---

# 95. Do Not Overclaim Location Use

If permission is technically required but precise location is not used/shared, say so accurately.

---

# 96. Android NFC

No permission flow should be invented if platform does not require one.

Use platform capability state.

---

# 97. Android Lifecycle

Discovery should react to:

```text
foreground
background
screen changes
```

Stop expensive scanning when UI no longer needs it unless active link session requires continuation.

---

# 98. Process Death

Rust durable/session state determines whether pairing can resume.

Compose does not reconstruct trust from `SavedStateHandle`.

---

# 99. SavedStateHandle

May store:

```text
current screen
session ID
manual code draft
```

but not trust completion.

---

# 100. Desktop QR Scanner

Options:

```text
webcam
paste code
open QR image
```

---

# 101. Desktop v1 Recommendation

Support:

```text
show own QR
paste/manual code
optional webcam scan
```

Webcam scanning should not block shipping.

---

# 102. Desktop Clipboard

Copy:

```text
invite code
fingerprint
```

through explicit action.

---

# 103. Open QR Image

If supported:

```text
file picker
→ image decoder
→ QR parser
```

with bounded image processing.

---

# 104. Desktop Nearby

Could rely on:

```text
LAN
Bluetooth if available
```

behind same Rust proximity API.

---

# 105. No Hardware Assumption

UI only shows:

```text
Nearby unavailable on this device
```

if no suitable adapter.

---

# 106. Discovery Capability Snapshot

```rust
pub struct ProximityCapabilityView {
    pub nearby_available: bool,
    pub qr_scan_available: bool,
    pub nfc_available: bool,
    pub manual_code_available: bool,
}
```

---

# 107. Capability-Driven UI

Hide unsupported permanent capabilities.

Disable temporarily unavailable capabilities with reason.

---

# 108. Nearby Candidate Expiry

If candidate disappears:

```text
remove after grace period
```

or mark:

```text
No longer nearby
```

briefly.

---

# 109. Candidate List Stability

Do not reorder constantly from RSSI fluctuations.

Sort by:

```text
stable first-seen / semantic proximity class
```

with hysteresis.

---

# 110. Duplicate Discovery

Same physical candidate seen over BLE + LAN:

```text
one candidate
```

after Rust correlation where safe.

---

# 111. Correlation Privacy

Do not correlate unrelated observations beyond security/privacy model.

---

# 112. Nearby Candidate Selection Race

If candidate expires after tap:

```text
Device is no longer nearby
Try again
```

---

# 113. Multiple Simultaneous Candidates

Clearly show target identity at confirmation.

Avoid accidental approval of wrong nearby device.

---

# 114. User Confirmation Screen

Before trust-changing action show:

```text
name
avatar
SAS/fingerprint
relationship being created
```

---

# 115. Device Link Confirmation

Must explicitly say:

```text
This is one of my devices
```

---

# 116. Contact Confirmation

Must explicitly say:

```text
Add this person as a contact
```

---

# 117. Verification Confirmation

Must explicitly say:

```text
Verify this identity
```

---

# 118. Prevent Context Confusion

Use different:

```text
screen titles
icons
button labels
help text
```

for contact vs own-device flows.

---

# 119. QR Sharing

Android:

```text
system share sheet
```

Desktop:

```text
copy code
copy/save QR image optional
```

---

# 120. QR Export Privacy

Invite payload may grant capability.

UI should warn if sharing broadly.

---

# 121. Contact Invite Warning

Example:

```text
Anyone with this invite can send you a contact request until it expires.
```

Exact wording matches backend semantics.

---

# 122. Device Link QR Warning

Stronger:

```text
Do not share this code. It can be used to link a device to your account until it expires.
```

---

# 123. Single-Use Link

Show:

```text
Single use
```

if true.

---

# 124. Revoking Link Session

User can tap:

```text
Cancel link code
```

---

# 125. Pairing Session List

Devices/security screen could show active:

```text
Pending device link
```

if session persists.

---

# 126. No Hidden Pending Authorization

All active device-link sessions should be inspectable/cancellable.

---

# 127. Trust Transcript

Advanced security details can expose:

```text
method
time
session
```

but not raw secrets.

---

# 128. Identity Fingerprint

Part 08.

Nearby/QR flow routes to same contact identity semantics after authentication.

---

# 129. QR Contact Acceptance from Unknown Sender

Do not auto-create trusted relationship until user confirms.

---

# 130. Group Invite QR

Part 09 receives validated payload.

---

# 131. Extension QR Types

Future plugins may register typed QR payloads only through safe extension registry.

---

# 132. Unknown Plugin QR

Show:

```text
This code requires plugin X
```

only after safe type identification.

---

# 133. QR Bomb / Oversize Data

Rust parser enforces:

```text
size
depth
field bounds
```

---

# 134. Image Decoder Safety

QR image import is bounded/sandboxed where possible.

---

# 135. NFC Payload Bounds

Same.

---

# 136. Nearby Protocol Bounds

Untrusted discovery frames are bounded.

UI never allocates based on advertised length.

---

# 137. Security Error Taxonomy

```rust
pub enum BootstrapFailureKind {
    Expired,
    Revoked,
    Replay,
    InvalidPayload,
    IdentityMismatch,
    SasMismatch,
    UnsupportedVersion,
    CandidateUnavailable,
    PermissionRequired,
    AuthorizationDenied,
    Internal,
}
```

---

# 138. User-Facing Mapping

Expired:

```text
Code expired
```

Replay/revoked:

```text
Code is no longer valid
```

Mismatch:

```text
Identity could not be verified
```

Unsupported:

```text
Update one of the devices and try again
```

---

# 139. Do Not Expose Security Oracle Details

Avoid telling attacker exactly which cryptographic field failed.

---

# 140. Version Mismatch

If safe:

```text
This device uses an incompatible version
Update the app
```

---

# 141. Downgrade Protection

UI does not offer:

```text
Continue insecurely
```

if Rust security policy rejects version.

---

# 142. Identity Changed During Pairing

Abort.

Show:

```text
Identity changed during verification
Start again
```

---

# 143. Session Timeout

Show:

```text
Session expired
```

with:

```text
Create new code
```

---

# 144. Accessibility — QR

QR is never the only path.

Always offer:

```text
Enter code manually
Compare code
```

where protocol supports.

---

# 145. Screen Reader

QR screen announces:

```text
Contact invite QR code. Expires in 4 minutes. Manual code 83 14 22.
```

where safe.

---

# 146. Manual Code Grouping

Use large grouped digits/words.

---

# 147. Color Independence

Verified/mismatch/expired state uses:

```text
icon
text
```

not color only.

---

# 148. Large Font

Codes wrap/resize without truncation.

---

# 149. RTL

UI layout mirrors.

Cryptographic/manual codes preserve canonical order.

---

# 150. Reduced Motion

Nearby scanning animation can become static progress/status.

---

# 151. Haptics Android

Useful sparingly for:

```text
QR recognized
NFC detected
verification success
mismatch
```

---

# 152. Audio Cues

Optional accessibility setting for successful scan.

---

# 153. Desktop Keyboard

Nearby list:

```text
Up/Down
Enter
```

QR/manual code dialog fully keyboard accessible.

---

# 154. Focus After Scan

Move focus to:

```text
identity confirmation
```

not hidden scanner control.

---

# 155. TalkBack Scanner

Scanner screen needs:

```text
instructions
manual-code alternative
close
```

Do not require visually aligning QR without alternative.

---

# 156. Pairing History

Contact/security details can show completed verification history.

Do not keep unnecessary failed-session logs indefinitely.

---

# 157. Failed Session Privacy

Bound retention.

---

# 158. Telemetry

Never log:

```text
full invite code
SAS
QR payload
fingerprint
```

---

# 159. Safe Metrics

Possible:

```text
method used
success/failure class
time to complete
permission denial rate
```

without identifiers/codes.

---

# 160. Crash Reports

Redact bootstrap payloads.

---

# 161. Nearby Diagnostics

Advanced support bundle may include:

```text
adapter state
discovery method
failure reason code
```

without peer identity/secrets.

---

# 162. Contact Pairing Presentation API

```rust
pub trait ContactPairingPresentation {
    async fn create_invite(
        &self,
        policy: ContactInvitePolicy,
    ) -> Result<ContactInviteView, UiError>;

    async fn inspect_invite(
        &self,
        payload: BootstrapPayload,
    ) -> Result<ContactInvitePreview, UiError>;

    async fn accept_invite(
        &self,
        payload: BootstrapPayload,
    ) -> Result<ContactPairingResult, UiError>;

    async fn cancel_invite(
        &self,
        invite: ContactInviteId,
    ) -> Result<(), UiError>;
}
```

---

# 163. Verification Presentation API

```rust
pub trait PairVerificationPresentation {
    async fn begin(
        &self,
        account: AccountId,
        method: VerificationMethod,
    ) -> Result<VerificationSessionView, UiError>;

    async fn confirm_sas(
        &self,
        session: VerificationSessionId,
        matches: bool,
    ) -> Result<VerificationResultView, UiError>;

    async fn process_qr(
        &self,
        payload: BootstrapPayload,
    ) -> Result<VerificationResultView, UiError>;

    async fn cancel(
        &self,
        session: VerificationSessionId,
    ) -> Result<(), UiError>;
}
```

---

# 164. Nearby Presentation API

```rust
pub trait NearbyPresentation {
    async fn capabilities(
        &self,
    ) -> Result<ProximityCapabilityView, UiError>;

    async fn start_discovery(
        &self,
    ) -> Result<NearbyDiscoverySessionId, UiError>;

    async fn stop_discovery(
        &self,
        session: NearbyDiscoverySessionId,
    ) -> Result<(), UiError>;

    async fn connect(
        &self,
        candidate: NearbyCandidateId,
    ) -> Result<NearbyPairingSessionView, UiError>;
}
```

---

# 165. Device Link Presentation API

```rust
pub trait DeviceLinkPresentation {
    async fn create_invite(
        &self,
    ) -> Result<DeviceLinkInviteView, UiError>;

    async fn inspect_invite(
        &self,
        payload: BootstrapPayload,
    ) -> Result<DeviceLinkPreview, UiError>;

    async fn approve(
        &self,
        session: DeviceLinkSessionId,
    ) -> Result<(), UiError>;

    async fn deny(
        &self,
        session: DeviceLinkSessionId,
    ) -> Result<(), UiError>;

    async fn confirm_sas(
        &self,
        session: DeviceLinkSessionId,
        matches: bool,
    ) -> Result<(), UiError>;

    async fn cancel(
        &self,
        session: DeviceLinkSessionId,
    ) -> Result<(), UiError>;
}
```

---

# 166. Platform Scanner Interface

```rust
pub enum PlatformBootstrapInput {
    QrPayload(Vec<u8>),
    NfcPayload(Vec<u8>),
    ManualCode(String),
}
```

Platform UI obtains input.

Rust validates.

---

# 167. Do Not Give Camera/NFC to Domain Core

Domain receives payload, not Activity/camera objects.

---

# 168. Android ViewModel

Owns:

```text
scanner visibility
permission effects
manual-code text
selected candidate
confirmation sheets
```

Rust owns:

```text
candidate/auth/session/trust state
```

---

# 169. Dioxus Presenter

Owns:

```text
nearby selection
manual-code field
QR window
webcam/paste flow
focus
```

---

# 170. Nearby Events

```rust
pub enum NearbyUiEvent {
    CandidateAdded(NearbyCandidateView),
    CandidateUpdated(NearbyCandidateView),
    CandidateRemoved(NearbyCandidateId),
    DiscoveryStateChanged(NearbyDiscoveryState),
}
```

---

# 171. Verification Events

```rust
pub enum VerificationUiEvent {
    SessionChanged(VerificationSessionView),
    VerificationCompleted(VerificationResultView),
    VerificationFailed(BootstrapFailureKind),
}
```

---

# 172. Device Link Events

```rust
pub enum DeviceLinkUiEvent {
    SessionChanged(DeviceLinkSessionView),
    LinkCompleted(DeviceSummaryView),
    LinkFailed(BootstrapFailureKind),
}
```

---

# 173. Event Locality

Only update current pairing flow/candidate.

Do not refresh whole contacts/devices screens unnecessarily.

---

# 174. Pairing Session Persistence

Sensitive session state remains Rust-owned.

UI can reconnect by session ID.

---

# 175. Expired Session Reattach

Rust returns:

```text
Expired
```

UI creates fresh flow.

---

# 176. Parallel Pairing

Limit concurrent high-risk device-link sessions.

---

# 177. Contact Invites

Multiple may exist if product allows.

---

# 178. Own-Device Link Limit

Recommendation:

```text
one or very few active link sessions
```

to reduce confusion.

---

# 179. Pairing Rate Limits

Backend security policy.

UI can show:

```text
Try again later
```

---

# 180. Abuse Protection

Unknown nearby devices cannot force modal dialogs.

They only appear in nearby list.

---

# 181. Unsolicited NFC

Requires deliberate user interaction/session where possible.

---

# 182. Unsolicited QR

Scanner requires user opening scanner.

---

# 183. Contact Request After Scan

User remains in control.

No silent message permission.

---

# 184. Own Device Security

Device-link flow should be accessible from:

```text
Devices
Security Center
```

not general contact add shortcut alone.

---

# 185. Device Link Audit

After link:

```text
New device added
```

security event appears on existing devices.

---

# 186. Device Link Notification

Other trusted devices may be notified.

---

# 187. Unauthorized Attempt

If denied/failed:

```text
Device was not linked
```

and security system may log event.

---

# 188. Revoke Newly Linked Device

Devices screen allows immediate revoke.

---

# 189. Device Naming

After link, user can set friendly local/account device name:

```text
Pixel 10
Desktop
Laptop
```

---

# 190. Name Is Not Device Identity

`DeviceId` remains authority.

---

# 191. Device Type Icon

Android phone/desktop/tablet icons are informational only.

---

# 192. Contact Nearby Reverification

Existing contact discovered nearby can show:

```text
Verify Alice nearby
```

after authenticated identity match.

---

# 193. Identity Mismatch Existing Contact

Strong warning.

Do not silently create second contact with same display name.

---

# 194. Duplicate Nearby Candidates

Rust should deduplicate authenticated same identity.

---

# 195. Contact Verification Success Navigation

Return to contact profile/conversation.

---

# 196. Device Link Success Navigation

Return to:

```text
Devices
```

not Contacts.

---

# 197. Group Invite Success Navigation

Return to group.

---

# 198. Typed Result Routing

Bootstrap result:

```rust
pub enum BootstrapResult {
    ContactAdded(AccountId),
    ContactVerified(AccountId),
    DeviceLinked(DeviceId),
    GroupJoined(GroupId),
}
```

UI routes accordingly.

---

# 199. No Generic Success Screen

Use context-specific success message.

---

# 200. Empty Nearby State

```text
No nearby devices found
```

Actions:

```text
Try again
Scan QR
Enter Code
```

---

# 201. Nearby Unavailable

```text
Nearby discovery isn't available on this device
```

Fallbacks remain.

---

# 202. Bluetooth Off

Android can show:

```text
Turn on Bluetooth
```

via platform settings/action where needed.

---

# 203. Wi-Fi Off

Only request enabling if current method requires it.

---

# 204. Airplane Mode

Local Bluetooth/Wi-Fi behavior varies.

UI should use capability state, not assume total impossibility.

---

# 205. Camera Unavailable

Offer manual code.

---

# 206. NFC Off

Offer:

```text
Turn on NFC
```

or QR.

---

# 207. Pairing Loading

Use semantic steps:

```text
Connecting
Verifying
Waiting for confirmation
Authorizing
```

not indefinite spinner.

---

# 208. Cancellation

Every waiting screen exposes:

```text
Cancel
```

---

# 209. Back Navigation Android

Back cancels/dismisses only current UI flow.

If cancelling security session is necessary:

```text
ViewModel asks Rust cancel
```

---

# 210. Desktop Escape

Esc closes current modal after safe cancellation.

---

# 211. Background Android

If user leaves scanner:

```text
stop camera
```

Pairing session may remain if already authenticated.

---

# 212. Foreground Return

Request fresh session snapshot.

---

# 213. Screen Rotation

Does not duplicate scan acceptance.

Use session IDs.

---

# 214. Duplicate Scanner Event

Same QR may be decoded many frames.

Deduplicate by payload/session.

---

# 215. Haptic/Visual Scan Confirmation

Once recognized:

```text
freeze scanner
show processing
```

to avoid repeated scans.

---

# 216. Manual Entry Paste

If pasted code includes spaces/hyphens:

```text
normalize safely
```

in parser.

---

# 217. Copy Code

Display expiration next to code so users understand lifetime.

---

# 218. QR Brightness

Android may optionally offer:

```text
increase screen brightness
```

while showing QR.

Restore afterward.

---

# 219. QR Screen Sleep

Keep screen awake briefly while actively showing pairing QR if appropriate.

---

# 220. Privacy Against Shoulder Surfing

Device-link codes should expire quickly.

Contact QR may have less-sensitive semantics but still bounded.

---

# 221. Screenshot Testing

Required states:

```text
nearby searching
candidate found
connecting
verification
SAS
match
mismatch
QR contact
QR device link
expired QR
permission denied
manual code
device approval
link complete
dark mode
large font
RTL
```

---

# 222. Android Test Matrix

Verify:

```text
camera permission
Bluetooth/nearby permission
NFC
QR duplicate frames
rotation
background/foreground
process death
TalkBack
manual-code fallback
```

---

# 223. Desktop Test Matrix

Verify:

```text
nearby list
keyboard selection
manual code
paste code
QR display
optional webcam scanner
focus
```

---

# 224. Security Test Matrix

Verify:

```text
expired invite
replay
revoked invite
wrong SAS
identity mismatch
unsupported version
self scan
device already linked
denied authorization
```

---

# 225. Offline Tests

Verify QR/SAS/local pairing works without Internet when backend path supports it.

---

# 226. Multi-Device Test

Link new Android device from desktop.

Expected:

```text
new unique DeviceId
security event
new device appears in Devices
```

---

# 227. Contact vs Device Confusion Test

Ensure a device-link QR can never render ordinary:

```text
Add Contact
```

confirmation.

---

# 228. QR Type Confusion Test

Group/contact/device QR cannot be interpreted as another type.

---

# 229. Accessibility Test

Complete contact verification without camera using manual/SAS flow.

---

# 230. Performance

Nearby UI update rates must be bounded.

RSSI changes should not produce 60 recompositions/sec.

---

# 231. Discovery Event Coalescing

Coalesce candidate signal/proximity changes.

---

# 232. Scanner Performance

QR decoding should run off main UI thread where appropriate.

---

# 233. Battery

Discovery/camera/NFC sessions are short-lived and user-driven.

---

# 234. Initial Production Scope

Ship:

```text
Nearby discovery abstraction
contact invite QR
contact verification QR/SAS
manual code
Android QR camera
Android NFC where practical
desktop QR display/manual/paste
own-device linking as separate flow
explicit device authorization
session expiry/replay handling
offline local pairing where supported
```

Defer:

```text
continuous ambient discovery
automatic contact exchange
ultra-wideband precise distance
complex social proximity
background NFC workflows
```

---

# 235. Definition of Done

UI/UX Part 12 is complete when:

- nearby observation is clearly distinct from authenticated identity
- contact addition, contact verification, own-device linking, and group invites use separate typed flows
- QR payload type is authenticated/validated by Rust
- device-link QR is visually and semantically distinct from contact QR
- own-device authorization requires explicit approval
- SAS comparison has explicit Match/Mismatch actions
- mismatches, expiry, replay, revocation, and version errors cannot result in trust
- Android camera, nearby, NFC, lifecycle, and permission behavior are defined
- desktop manual/paste/QR and optional webcam flows are defined
- QR/manual/SAS accessibility fallbacks are available
- session and trust state remain Rust-owned across lifecycle/process changes
- offline pairing is treated as valid where local transports allow
- no raw Bluetooth/Wi-Fi transport detail leaks into normal UX
- nearby candidate lists are stable, bounded, and privacy-aware
- security-sensitive codes/payloads are excluded from telemetry/logs
- typed Rust presentation APIs for contact pairing, verification, nearby discovery, and device linking are defined
- contact/device/group result routing uses distinct stable IDs
- security, offline, lifecycle, accessibility, and type-confusion test matrices are specified

---

# 236. Final Architecture

```text
                   PLATFORM INPUTS
       ┌──────────────┼──────────────┐
       │              │              │
     Nearby          QR             NFC
       │              │              │
       └──────────────┼──────────────┘
                      │
             Rust Bootstrap Core
                      │
       ┌──────────────┼───────────────┐
       │              │               │
 Contact Pairing  Verification   Device Linking
       │              │               │
       └──────────────┼───────────────┘
                      │
              Typed Result
       ┌──────────────┼───────────────┐
       │              │               │
   AccountId      Verified        DeviceId
                  AccountId
```

Platform UI:

```text
Desktop Dioxus:
    nearby list
    QR display
    manual/paste code
    optional webcam scan

Android Compose:
    nearby
    QR camera
    NFC
    manual code
    platform permissions
```

---

# 237. Final Principle

The proximity/bootstrap UX should make secure pairing feel simple without pretending that proximity itself creates trust.

The correct mental model is:

```text
I can see this device nearby
→ I authenticate who it is
→ I explicitly choose what relationship to create
```

and, for own devices:

```text
I scanned a device-link code
→ I verify the candidate
→ I explicitly authorize it into my account
```

not:

```text
it is nearby, therefore it is trusted
```

This preserves the security architecture while giving Dioxus desktop and Android Compose clear, native, offline-capable pairing flows.
