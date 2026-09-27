# UI/UX Part 08 — Contacts, Requests, Verification & Identity UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 08  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete contacts, message requests, profile, identity verification, trust, blocking, reporting, nearby discovery, QR/SAS verification, own-device linking differentiation, and identity-change UX across desktop and Android.

---

# 1. Purpose

A communication product must clearly separate:

```text
someone I know
someone I accepted
someone I verified
someone nearby
one of my own devices
someone blocked
an unknown requester
```

If these concepts are blurred, users can make dangerous assumptions.

The governing principle is:

> **Contact familiarity, conversation acceptance, cryptographic verification, and device trust are distinct concepts and must remain visually and semantically distinct.**

---

# 2. Architectural Position

```text
Rust Identity / Trust Core
        │
        ├── Account identity
        ├── Device identity
        ├── Contact metadata
        ├── Verification state
        ├── Requests
        ├── Block state
        └── Revocation state
        │
        ▼
Identity Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Primary Identity Concepts

```rust
pub enum ContactRelationship {
    Unknown,
    Requested,
    Accepted,
    SavedContact,
    Verified,
    Blocked,
}
```

Device trust remains separate.

---

# 4. Do Not Collapse Trust States

Incorrect:

```text
saved contact = verified
```

Correct:

```text
saved contact
+
verification state
```

---

# 5. Core Contact Summary

```rust
pub struct ContactSummary {
    pub account: AccountId,
    pub display_name: String,
    pub avatar: AvatarRef,
    pub relationship: ContactRelationship,
    pub verification: VerificationState,
    pub presence: Option<PresenceSummary>,
    pub unread_requests: u32,
}
```

---

# 6. Verification States

```rust
pub enum VerificationState {
    Unverified,
    Verified,
    IdentityChanged,
    Revoked,
    Unknown,
}
```

---

# 7. Contact List Destinations

Recommended:

```text
Contacts
Requests
Nearby
Blocked
```

Optional:

```text
Organizations
Favorites
```

later.

---

# 8. Desktop Contacts Layout

Recommended:

```text
Left sidebar:
    Contacts
    Requests
    Nearby
    Blocked

Main list/detail:
    contact list
    profile detail
```

---

# 9. Android Contacts Layout

Phone:

```text
TopAppBar
Search
Contacts list
FAB / Add Contact
```

Secondary:

```text
Requests
Nearby
Blocked
```

via tabs/chips/menu depending density.

---

# 10. Tablet/Foldable Contacts

Use:

```text
NavigationRail
Contact List
Contact Detail
```

---

# 11. Contact Row Anatomy

Recommended:

```text
avatar
display name
verification badge if meaningful
presence optionally
subtitle/status
```

Avoid too many trust icons.

---

# 12. Verification Badge

Show:

```text
Verified
```

as explicit semantic state.

Do not rely on checkmark color alone.

---

# 13. Identity Changed Warning

This is high priority.

Show:

```text
Identity changed
Review
```

not subtle icon only.

---

# 14. Presence

Optional small status.

Do not imply trust.

---

# 15. Add Contact Entry Points

Potential:

```text
Search by known identifier
Scan QR
Nearby
Shared invite
Manual code
```

---

# 16. Contact Search

If global directory does not exist:

```text
search local contacts only
```

Do not imply cloud username discovery unless backend supports it.

---

# 17. Add via QR

Flow:

```text
Scan QR
→ Parse invite
→ Authenticate payload
→ Show identity preview
→ Verify/Accept
→ Save contact
```

---

# 18. Android QR UX

Compose launches camera/scanner.

Rust validates payload.

---

# 19. Desktop QR UX

Potential:

```text
webcam scan
paste code
open image
```

Initial desktop v1 may prioritize:

```text
paste code / show own QR
```

if camera scanning is less important.

---

# 20. Own Contact QR

User can show:

```text
My contact QR
```

for another person.

---

# 21. Own-Device QR

Must look different from contact QR.

Label clearly:

```text
Link another device
```

not:

```text
Add contact
```

---

# 22. Device Link vs Contact Link

Hard UX distinction.

Own-device flow may grant:

```text
account authorization
history sync
device trust
```

which is much more sensitive than contact addition.

---

# 23. Contact Invite Preview

Before acceptance show:

```text
display name
avatar
identity fingerprint short form
invite expiry
source
```

---

# 24. Fingerprint

Full cryptographic fingerprint belongs in verification/details screen.

Not normal list row.

---

# 25. Short Fingerprint

Can show grouped code:

```text
A7F2 19CC 8E42 ...
```

---

# 26. Safety Number / SAS

Verification flow may compare:

```text
short numeric/string code
```

---

# 27. Verification Methods

```rust
pub enum VerificationMethod {
    QrScan,
    SasCompare,
    FingerprintCompare,
    TrustedDeviceIntroduction,
}
```

---

# 28. Verification Flow

Recommended:

```text
Open Contact
→ Verify Identity
→ choose method
→ compare/scan
→ Rust validates transcript
→ confirmation
→ Verified
```

---

# 29. Verification Is Explicit

Do not automatically mark verified because:

```text
invite was accepted
message delivered
peer is nearby
```

---

# 30. QR Verification

If both users are physically together:

```text
scan each other's authenticated QR
```

best UX.

---

# 31. SAS Compare

Show:

```text
same code on both devices?
```

Actions:

```text
Matches
Doesn't match
```

---

# 32. Do Not Use Ambiguous Confirmation

Bad:

```text
OK
```

Good:

```text
Codes match
Codes do not match
```

---

# 33. Verification Success

Show:

```text
Identity verified
```

with date/device context optionally.

---

# 34. Verification History

Contact details can show:

```text
Verified on date
Verification method
```

without exposing unnecessary secrets.

---

# 35. Identity Change

If account/device root identity changes unexpectedly:

```text
strong warning
```

---

# 36. Identity Change Screen

Show:

```text
Previous identity no longer matches
Messages/calls may be blocked by security policy
Verify again before continuing
```

Actions:

```text
Verify again
Trust new identity
Block
View details
```

---

# 37. Trust New Identity

High-risk action.

Require:

```text
explicit confirmation
```

Possibly device authentication in high-security mode.

---

# 38. Identity Change Cannot Be Silently Cleared

Hard rule.

---

# 39. Contact Profile Screen

Recommended sections:

```text
Identity
Message
Audio Call
Video Call
Shared Media
Verification
Devices summary if appropriate
Notifications
Block
Delete Contact
```

---

# 40. Profile Header

Shows:

```text
avatar
display name
relationship
verification
presence
```

---

# 41. Contact Alias

User may assign local display name.

This does not change cryptographic identity.

---

# 42. Alias Editing

Action:

```text
Edit contact name
```

Stored locally/account preference.

---

# 43. Claimed vs Local Name

If needed in security details:

```text
Local name: Alice
Claimed account name: alice@example
```

---

# 44. Profile Status Text

Optional user status/about.

Treat as untrusted display content.

---

# 45. Shared Media

Link to Part 10 files/media view scoped to contact/conversation.

---

# 46. Notification Settings

Per-contact/conversation mute settings.

---

# 47. Block

Blocking should:

```text
stop new messages
stop calls
stop presence sharing
stop typing/read receipts
```

according to backend policy.

---

# 48. Block UX

Action in contact details and request sheet.

Confirmation:

```text
Block Alice?
```

Explain consequences briefly.

---

# 49. Blocked State

Profile shows:

```text
Blocked
Unblock
```

Composer/call actions disabled.

---

# 50. Unblock

Explicit action.

Does not automatically:

```text
verify
accept request
restore deleted history
```

---

# 51. Delete Contact

Deleting contact metadata is distinct from:

```text
blocking
deleting conversation
revoking identity
```

---

# 52. Delete Contact UX

Explain:

```text
removes saved contact information
does not block them
```

---

# 53. Requests

Unknown users/messages go to request area.

---

# 54. Request Summary

```rust
pub struct ContactRequestView {
    pub request_id: RequestId,
    pub account: AccountId,
    pub display_name: String,
    pub avatar: AvatarRef,
    pub preview: RequestPreview,
    pub received_at: Timestamp,
    pub trust_hint: RequestTrustHint,
}
```

---

# 55. Request Preview

Privacy-conscious default:

```text
New message request
```

or limited excerpt.

---

# 56. Request Detail

Show:

```text
sender identity preview
first message
mutual context if available
verification status
```

---

# 57. Request Actions

```text
Accept
Delete
Block
Report
```

---

# 58. Accept

Creates accepted conversation/contact relationship.

Still:

```text
Unverified
```

until verification.

---

# 59. Delete Request

Removes request locally according to backend semantics.

---

# 60. Block Requester

Blocks identity and removes request.

---

# 61. Report

Only if abuse/reporting system exists.

---

# 62. Reporting Privacy

User chooses what evidence is shared.

Do not auto-upload full conversation.

---

# 63. Report Flow

Potential:

```text
Reason
Optional selected messages
Submit
```

---

# 64. Request Flood UX

Do not show hundreds of push-like cards.

Use:

```text
request count
paged list
rate-limited notifications
```

---

# 65. Unknown Request Read Receipts

Default:

```text
do not send
```

until accepted.

---

# 66. Unknown Request Typing

Default:

```text
do not send
```

---

# 67. Unknown Request Presence

Default:

```text
do not share
```

---

# 68. Nearby Discovery

Part 14 proximity.

Nearby list is:

```text
discovery observation
```

not trusted identity.

---

# 69. Nearby Row

Show:

```text
Nearby device/person
signal proximity hint
verification state
```

Avoid exact distance unless reliable.

---

# 70. RSSI

Do not show raw RSSI to normal users.

Advanced diagnostics only.

---

# 71. Nearby Discovery States

```rust
pub enum NearbyPeerState {
    Discovered,
    Connecting,
    Authenticating,
    Verified,
    Failed,
}
```

---

# 72. Nearby Identity Safety

Before authentication:

```text
Unverified nearby device
```

---

# 73. Nearby Add Flow

```text
Select
→ authenticate
→ compare/confirm identity
→ save contact
```

---

# 74. Nearby Does Not Mean Trusted

Hard rule.

---

# 75. Bluetooth/Wi-Fi Details

Normal UI should say:

```text
Nearby
```

not:

```text
BLE transport
Wi-Fi Aware
```

Diagnostics can expose path.

---

# 76. Own Device Nearby

If platform detects own-device link candidate:

```text
Link your device
```

not normal contact flow.

---

# 77. Device Linking Entry

Dedicated:

```text
Devices
→ Link new device
```

---

# 78. Own Device Verification

Can use:

```text
QR
SAS
existing trusted device approval
```

---

# 79. Device Link Warning

Explain:

```text
This device may access your messages and account
```

---

# 80. Device Authorization

Requires explicit approval.

---

# 81. Device Link Success

Show:

```text
Device linked
```

and route to Devices screen.

---

# 82. Device Link Failure

Examples:

```text
Code expired
Already used
Identity mismatch
Unsupported version
```

---

# 83. Contact vs Device Visual Language

Use different icons/labels.

Do not reuse identical "verified" badge semantics blindly.

---

# 84. Contact Verification

Means:

```text
peer/account identity verified
```

---

# 85. Device Trust

Means:

```text
this device is authorized under my account
```

---

# 86. Group Member Identity

Group member list should show:

```text
display name
role
verification state if relevant
```

---

# 87. Group Verification Density

Do not show 100 verification icons by default.

Could show warning only when:

```text
identity changed
unknown member
```

---

# 88. Member Detail

Tap member:

```text
profile
message
call
verify
block
```

depending policy.

---

# 89. Organization Identity

Future managed deployments may have:

```text
Organization verified
Managed account
```

separate from personal verification.

---

# 90. Directory Results

If organization directory exists:

```text
Directory contact
```

does not necessarily mean verified E2EE identity.

---

# 91. Search Contacts

Local search:

```text
name
alias
organization
```

---

# 92. Contact List Sorting

Recommended:

```text
Favorites/Recent? optional
Alphabetical
```

---

# 93. Recommendation

Default alphabetical list.

Search handles large sets.

---

# 94. Favorites

Optional later.

Avoid excessive inbox-like ranking.

---

# 95. Recent Contacts

Calls/chats already provide recency.

No need separate unless useful.

---

# 96. Contact Import

Future:

```text
Android contacts
CSV
vCard
```

must not automatically create verified identities.

---

# 97. Android Contacts Permission

Avoid requesting full system contacts unless feature actually uses it.

---

# 98. Contact Matching Privacy

If later matching phone/email contacts:

```text
privacy-sensitive
```

requires separate backend/privacy architecture.

---

# 99. Manual Identifier Entry

Potential:

```text
account code
public key/fingerprint
invite code
```

---

# 100. Identifier Validation

Rust authoritative.

---

# 101. Invalid Identifier

Show inline:

```text
This code is not valid
```

---

# 102. Expired Invite

Show:

```text
This invite has expired
Request a new one
```

---

# 103. Already Added

Show:

```text
Already in contacts
Open contact
```

---

# 104. Self Contact

If user scans own contact code:

```text
This is your identity
```

offer:

```text
Link device
```

only if appropriate.

---

# 105. Duplicate Contact

Same AccountId must not create duplicate logical contact.

Aliases may merge.

---

# 106. Contact Merge

Future for imported address book.

Not needed for crypto identity core.

---

# 107. Identity Fingerprint Screen

Show:

```text
full fingerprint
QR
copy
share
verification state
```

---

# 108. Fingerprint Formatting

Group for readability.

---

# 109. Copy Fingerprint

Explicit action.

---

# 110. Share Fingerprint

Can use OS share sheet.

---

# 111. QR Content

Should encode authenticated verification payload, not only pretty text.

---

# 112. QR Expiry

Verification/contact invite may expire.

UI displays expiry if relevant.

---

# 113. Screenshot of QR

May remain usable until expiry.

Security model must assume QR can be copied.

---

# 114. Single-Use Invite

UI should say:

```text
This code can be used once
```

if applicable.

---

# 115. Verification Confirmation

After QR/SAS:

```text
Verified
```

only after Rust confirms complete transcript.

---

# 116. Partial Verification

If flow interrupted:

```text
remain Unverified
```

---

# 117. Verification Cancellation

No trust state change.

---

# 118. Verification Error

Examples:

```text
Codes do not match
Session expired
Identity changed during verification
```

---

# 119. Verification Failure Severity

Mismatch should be strong:

```text
Do not continue
```

with block/retry options.

---

# 120. Identity Revoked

If contact identity/device is revoked:

```text
Revoked
```

and communication policy determines continuation.

---

# 121. Device Revocation Visibility

Contact details may show:

```text
one of Alice's devices was removed
```

only if product exposes device-level details.

Default:

```text
account-level trust only
```

---

# 122. Multi-Device Contact

Do not show every peer device in normal profile.

Advanced security details may.

---

# 123. Verified Account with New Device

Part 28 decides whether:

```text
new device needs verification
```

UI shows security warning only if policy requires.

---

# 124. Verification Badge Scope

Must be clear whether verification applies to:

```text
account root
specific device
current session
```

Recommendation UI:

```text
account-level verified
```

with device details hidden unless needed.

---

# 125. Safety Number Change

If root identity changes:

```text
verified badge removed
Identity changed warning
```

---

# 126. Contact Details Actions

Recommended:

```text
Message
Audio Call
Video Call
Search in Conversation
Shared Media
Verify Identity
Mute
Block
Delete Contact
```

---

# 127. Destructive Ordering

Place:

```text
Block
Delete Contact
```

in lower destructive section.

---

# 128. Desktop Profile Layout

Wide:

```text
header
actions row
details sections
shared media preview
security section
```

---

# 129. Android Profile Layout

Scrollable:

```text
large avatar/header
action buttons
sections/cards
destructive actions
```

---

# 130. Adaptive Tablet Profile

List/detail.

---

# 131. Call Availability

Disable call buttons if:

```text
blocked
calls disabled
unsupported
```

Rust capability.

---

# 132. Message Availability

Composer/open chat action similarly capability-driven.

---

# 133. Contact Capability DTO

```rust
pub struct ContactCapabilities {
    pub can_message: bool,
    pub can_audio_call: bool,
    pub can_video_call: bool,
    pub can_verify: bool,
    pub can_block: bool,
    pub can_report: bool,
}
```

---

# 134. Android Long Press Contact

Potential menu:

```text
Message
Call
Mute
Block
```

but primary tap should open profile/conversation depending product choice.

---

# 135. Recommendation

Tap contact row:

```text
open contact profile
```

from Contacts area.

Tap conversation row:

```text
open conversation
```

Keeps mental model clear.

---

# 136. Desktop Double Click

Optional:

```text
open conversation
```

while single click opens profile/detail.

Not required.

---

# 137. Requests Desktop

Use list + detail.

---

# 138. Requests Android

Full-screen list/detail or bottom sheet.

---

# 139. Nearby Android

Can include scanning animation/status.

Respect reduced motion.

---

# 140. Nearby Desktop

Simple list with:

```text
Searching nearby…
```

---

# 141. Discovery Toggle

User can start/stop nearby discovery.

Do not scan continuously without reason.

---

# 142. Battery Awareness

Android discovery UI may show:

```text
Searching…
```

only while active.

Rust handles scheduling.

---

# 143. Location Permission

If Android requires location-related permission for certain nearby APIs, Kotlin handles platform explanation.

Do not claim location is being shared if only permission is technical requirement.

---

# 144. Privacy Explanation

Explain accurately:

```text
Nearby permission is used to discover devices
```

not vague.

---

# 145. NFC Pairing

Android:

```text
Tap phones
```

if supported.

Rust validates payload/transcript.

---

# 146. NFC Fallback

Offer:

```text
Scan QR
```

---

# 147. Contact Request Notifications

Part 31 can show:

```text
New message request
```

with generic privacy.

---

# 148. Request Notification Tap

Opens request detail.

---

# 149. Blocked Contact Notifications

None.

---

# 150. Verification Notification

Potential:

```text
Identity changed
```

security alert.

---

# 151. Security Center Integration

Identity warnings link to Part 15 UI/UX security center later.

---

# 152. Device Screen Integration

Own-device linking routes to Devices.

---

# 153. Backup Integration

Verified contact state may be included in backup per Part 33.

---

# 154. Search Integration

Part 32 can search contacts.

---

# 155. Contact Search Result

Shows:

```text
name
verification
relationship
```

---

# 156. Presence Privacy

If user hides presence from peer:

```text
contact UI may still show peer presence if peer shares theirs
```

directionality matters.

---

# 157. Read Receipt Privacy

Per-contact setting may exist.

---

# 158. Typing Privacy

Per-contact or global.

Do not clutter profile initially unless advanced privacy page.

---

# 159. Notification Mute

Contact/profile may link to conversation notification settings.

---

# 160. Block Effects Explanation

Brief:

```text
They will no longer be able to message or call you through this account.
```

Exact semantics must match backend.

---

# 161. Report vs Block

Report can imply:

```text
share evidence
```

Block is local trust/action policy.

Do not merge.

---

# 162. Abuse Report Consent

Show exactly what content will be included.

---

# 163. Identity Details Privacy

Do not expose:

```text
peer IP
device hardware identifiers
transport addresses
```

in contact profile.

---

# 164. Device Names

If showing peer devices in advanced view:

```text
Alice's phone
Alice's desktop
```

only if peer/device metadata sharing policy allows.

---

# 165. Contact Notes

Future optional private notes.

If added:

```text
local/private
```

not sent to peer.

---

# 166. Custom Nickname

Local only.

---

# 167. Avatar Change

Updates profile/list.

Does not affect verification.

---

# 168. Profile Name Change

Likewise.

---

# 169. Spoofing Risk

Verified state should remain tied to cryptographic identity, not display name/avatar.

---

# 170. Accessibility — Contact Row

Screen reader:

```text
Alice, verified, online
```

or:

```text
Bob, unverified
```

---

# 171. Request Accessibility

Announce:

```text
Message request from Alice, unverified
```

---

# 172. Verification Accessibility

SAS code should be readable digit/group by digit/group.

---

# 173. QR Accessibility

Always provide alternate method:

```text
Compare code manually
```

---

# 174. Color Independence

Verified/warning/blocked states require icon/text, not color alone.

---

# 175. Large Font

Fingerprint/SAS screens must wrap cleanly.

---

# 176. RTL

Contact/profile layouts mirror correctly.

Fingerprint itself should preserve canonical order.

---

# 177. Reduced Motion

Nearby scanning animation can simplify.

---

# 178. Keyboard Desktop

Shortcuts:

```text
Ctrl/Cmd+N → Add/New contact or conversation depending context
Enter → open selected contact
Shift+F10 → menu
```

---

# 179. Desktop Focus

When contact list updates:

```text
preserve focus by AccountId
```

---

# 180. Android TalkBack

Contact actions expose:

```text
Message
Call
Verify
More
```

through accessible controls.

---

# 181. Screen Reader Verification

Announce state changes:

```text
Identity verified
Verification failed
```

---

# 182. Empty Contacts

```text
No contacts yet
```

Actions:

```text
Scan QR
Add Contact
Find Nearby
```

---

# 183. Empty Requests

```text
No message requests
```

---

# 184. Empty Nearby

```text
No nearby devices found
```

with:

```text
Try again
```

---

# 185. Empty Blocked

```text
No blocked contacts
```

---

# 186. Loading Contacts

Local contacts load immediately.

Nearby discovery is separate active loading state.

---

# 187. Offline Contacts

Local contacts/profile remain fully usable.

Verification/contact invite may require direct/local connectivity depending method.

---

# 188. Offline QR Verification

Should work if two devices can establish local authenticated exchange.

---

# 189. Request Acceptance Offline

If request already local:

```text
accept locally
```

and sync/send acknowledgement later if protocol supports.

---

# 190. Contact Alias Offline

Always local-first.

---

# 191. Verification Persistence

Verified state must survive app restart.

---

# 192. Identity Warning Persistence

Must remain until resolved.

---

# 193. Process Death Android

Compose reloads contact/trust snapshot from Rust.

No UI-local trust state.

---

# 194. Desktop Restart

Same.

---

# 195. Multi-Device Trust Sync

If verification is account-wide:

```text
verified contact state syncs
```

to user's devices securely.

---

# 196. Device-Local Verification

If architecture chooses per-device verification:

```text
UI must say so
```

Recommendation:

```text
account-level verification where cryptographic model permits
```

---

# 197. Block Sync

Recommendation:

```text
account-wide
```

across user's devices.

---

# 198. Alias Sync

Product choice.

Could be account-wide.

---

# 199. Contact Deletion Sync

Likewise.

---

# 200. Request Sync

Accepted/rejected state should synchronize to avoid duplicate requests.

---

# 201. Answered Elsewhere Equivalent

If request accepted on one device:

```text
other devices update
```

---

# 202. Rust Presentation API

```rust
pub trait ContactPresentation {
    async fn list(
        &self,
        filter: ContactFilter,
    ) -> Result<ContactPage, UiError>;

    async fn profile(
        &self,
        account: AccountId,
    ) -> Result<ContactProfileView, UiError>;

    async fn accept_request(
        &self,
        request: RequestId,
    ) -> Result<(), UiError>;

    async fn block(
        &self,
        account: AccountId,
    ) -> Result<(), UiError>;

    async fn unblock(
        &self,
        account: AccountId,
    ) -> Result<(), UiError>;

    async fn delete_contact(
        &self,
        account: AccountId,
    ) -> Result<(), UiError>;
}
```

---

# 203. Verification API

```rust
pub trait VerificationPresentation {
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
        payload: VerificationQrPayload,
    ) -> Result<VerificationResultView, UiError>;
}
```

---

# 204. Nearby API

```rust
pub trait NearbyPresentation {
    async fn start_discovery(&self) -> Result<(), UiError>;
    async fn stop_discovery(&self) -> Result<(), UiError>;
    async fn connect(&self, candidate: NearbyCandidateId) -> Result<(), UiError>;
}
```

---

# 205. Device Link API

Keep separate:

```rust
pub trait DeviceLinkPresentation {
    async fn create_link_invite(&self) -> Result<DeviceLinkInviteView, UiError>;
    async fn accept_link_invite(&self, payload: DeviceLinkPayload) -> Result<(), UiError>;
}
```

---

# 206. Separate APIs Matter

This reinforces:

```text
Contact verification ≠ own-device authorization
```

---

# 207. Contact Events

```rust
pub enum ContactUiEvent {
    ContactAdded(ContactSummary),
    ContactUpdated(ContactSummary),
    ContactRemoved(AccountId),
    RequestAdded(ContactRequestView),
    RequestRemoved(RequestId),
    VerificationChanged {
        account: AccountId,
        state: VerificationState,
    },
    BlockChanged {
        account: AccountId,
        blocked: bool,
    },
}
```

---

# 208. Android ViewModel

Owns:

```text
current tab/filter
search query
sheet/dialog state
permission effects
QR scanner effect
```

Rust owns trust/contact truth.

---

# 209. Dioxus Presenter

Owns:

```text
selection
sidebar filter
dialog/window state
focus
```

---

# 210. No Trust State in UI

Hard rule.

---

# 211. Security Error Mapping

Examples:

```text
VerificationMismatch
InviteExpired
IdentityChanged
AlreadyBlocked
UnauthorizedDeviceLink
```

---

# 212. Error UX

Mismatch:

```text
strong security warning
```

Expired:

```text
simple retry/new code
```

Permission:

```text
contextual platform request
```

---

# 213. Testing Matrix

Required:

```text
contact list
search
profile
request
accept
delete request
block
unblock
verify QR
verify SAS
mismatch
identity change
nearby discovery
own-device link distinction
offline
multi-device sync
```

---

# 214. Android Tests

Include:

```text
QR camera permission
NFC
nearby permission
TalkBack
large font
process death
deep link/invite
```

---

# 215. Desktop Tests

Include:

```text
keyboard
context menu
QR display/paste
multi-pane profile
focus preservation
```

---

# 216. Security Tests

Verify UI cannot mark contact verified without Rust confirmation.

---

# 217. Request Flood Test

Large unknown-request set remains bounded/paged.

---

# 218. Identity Change Test

Warning persists across restart and blocks/limits actions according to policy.

---

# 219. Device Link Confusion Test

Own-device flow never uses ordinary contact acceptance wording.

---

# 220. Verification Mismatch Test

UI never displays success after mismatch.

---

# 221. Block Test

After block:

```text
message/call controls disabled
presence/typing sharing stops according to backend
```

---

# 222. Multi-Device Block Test

Block on desktop updates phone if account-wide.

---

# 223. Accessibility Test

Full contact verification flow usable without:

```text
camera
color
gesture
```

through manual SAS/fingerprint path.

---

# 224. Initial Production Recommendation

For v1, ship:

```text
contact list
contact profile
message requests
accept/delete/block
QR contact invites
SAS/fingerprint verification
identity-change warning
blocked list
nearby discovery
own-device linking as separate flow
```

Defer:

```text
phone-number contact matching
cloud username directory
social graph
mutual contacts
complex organization directory
contact notes
favorites
```

---

# 225. Definition of Done

UI/UX Part 08 is complete when:

- contact familiarity, request acceptance, verification, blocking, and device trust are distinct
- verified state can only come from Rust security/identity subsystem
- request acceptance never implies verification
- own-device linking uses a separate flow/API from contact addition
- identity-change warnings are persistent and high-priority
- QR/SAS/fingerprint verification flows are defined
- nearby discovery is clearly marked unverified until authentication
- blocked contacts cannot message/call/share presence according to policy
- desktop Dioxus and Android Compose layouts are platform-native
- contact profile, request detail, blocked list, and nearby screens are defined
- multi-device trust/block/request synchronization behavior is explicit
- accessibility, manual verification fallback, RTL, large font, and keyboard/TalkBack behavior are defined
- contact, verification, nearby, and device-link Rust presentation APIs are separate and typed
- request flood, mismatch, identity change, block, offline, and multi-device cases are tested

---

# 226. Final Architecture

```text
                   RUST IDENTITY / TRUST CORE
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
     Contacts             Requests           Verification
        │                    │                    │
        └────────────────────┼────────────────────┘
                             │
                 Identity Presentation
                      ┌──────┴──────┐
                      │             │
                   Dioxus        Compose
                      │             │
               Desktop UX      Android UX
```

Separate sensitive path:

```text
Own Device Linking
      │
      ▼
DeviceLinkPresentation
      │
      ▼
Device Authorization
```

not:

```text
Contact Accept
```

---

# 227. Final Principle

The identity UX should help users understand exactly what they know and what they do not know.

The correct mental model is:

```text
I know this contact
≠
I accepted this request
≠
I verified this identity
≠
I authorized this device
```

By preserving those distinctions, the product can remain friendly while still communicating real cryptographic trust correctly across Dioxus desktop and Android Compose.
