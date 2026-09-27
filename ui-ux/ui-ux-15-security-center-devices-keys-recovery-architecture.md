# UI/UX Part 15 — Security Center, Devices, Keys & Recovery UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 15  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete security-center, trusted-device, key-health, recovery, re-authentication, device-revocation, compromise-response, sensitive-screen, recovery-key, backup-security, and security-event UX across desktop and Android.

---

# 1. Purpose

Security settings must make high-risk operations understandable without forcing users to learn cryptographic internals.

Users need to answer:

```text
Which devices can access my account?
Which devices are trusted?
Was a new device added?
Can I revoke a lost device?
Is my identity healthy?
Do I have a recovery method?
What happens if I lose all devices?
Why did a contact identity change?
Is my backup recoverable?
What should I do after suspicious activity?
```

The governing principle is:

> **The UI explains security consequences; Rust owns cryptographic truth, key state, authorization, revocation, and recovery validation.**

---

# 2. Architectural Position

```text
Rust Security / Identity Core
        │
        ├── account identity
        ├── device authorization
        ├── device revocation
        ├── key state
        ├── verification state
        ├── security events
        ├── recovery material
        └── backup/recovery policy
        │
        ▼
Security Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Main Security Center Sections

Recommended:

```text
Overview
Devices
Identity & Verification
Recovery
Backups
Security Events
Privacy
Advanced
```

---

# 4. Security Center Overview

The overview answers:

```text
Is there anything I need to act on?
```

Show:

```text
Security status
Trusted device count
Recovery status
Backup status
Recent important security events
```

---

# 5. Security Status

Recommended semantic states:

```rust
pub enum SecurityHealth {
    Healthy,
    Attention,
    Critical,
    Unknown,
}
```

---

# 6. Healthy

Examples:

```text
recovery configured
no unresolved identity warnings
no suspicious device events
```

---

# 7. Attention

Examples:

```text
no recovery configured
backup stale
new device not reviewed
```

---

# 8. Critical

Examples:

```text
identity compromise suspected
unknown authorized device
recovery material invalid
security verification failure
```

---

# 9. Do Not Turn Security into Gamification

Avoid:

```text
security score 82/100
```

unless truly meaningful.

Prefer explicit actionable state.

---

# 10. Security Overview Card

Example:

```text
Security looks good
4 trusted devices
Recovery configured
Last backup: yesterday
```

or:

```text
Security needs attention
New device linked 12 minutes ago
Review
```

---

# 11. Device Model

```rust
pub struct DeviceSecurityView {
    pub id: DeviceId,
    pub display_name: String,
    pub kind: DeviceKind,
    pub status: DeviceTrustState,
    pub added_at: Timestamp,
    pub last_active: Option<Timestamp>,
    pub current_device: bool,
    pub security_flags: Vec<DeviceSecurityFlag>,
}
```

---

# 12. Device Kinds

```rust
pub enum DeviceKind {
    AndroidPhone,
    AndroidTablet,
    Desktop,
    Laptop,
    ServerNode,
    Unknown,
}
```

Informational only.

---

# 13. Device Trust State

```rust
pub enum DeviceTrustState {
    Trusted,
    Pending,
    Revoked,
    Compromised,
    Unknown,
}
```

---

# 14. Devices Screen

Recommended list sections:

```text
This Device
Trusted Devices
Pending
Revoked / History
```

---

# 15. Device Row

Show:

```text
device name
device type
last active
trust state
current-device badge
```

Avoid raw IDs in normal list.

---

# 16. Current Device

Clearly label:

```text
This device
```

---

# 17. Device Details

Recommended:

```text
Device name
Device type
Added date
Last active
Authorization method
Security status
Device ID shortened
Revoke action
```

---

# 18. Device ID

Advanced/security detail.

Never use device display name as identity.

---

# 19. Rename Device

User may rename trusted device.

This changes display metadata only.

---

# 20. Device Last Active

Use privacy-conscious coarse wording:

```text
Active now
5 minutes ago
Yesterday
```

---

# 21. Pending Device

Pending means:

```text
authorization not complete
```

Must not have normal account access yet.

---

# 22. Pending Device Actions

```text
Approve
Deny
Cancel session
```

depending which device owns decision.

---

# 23. Device Link Integration

Part 12 owns linking mechanics.

Security Center shows:

```text
Link New Device
```

entry point.

---

# 24. New Device Security Event

After link:

```text
New device added
```

with:

```text
device name
time
review
revoke
```

---

# 25. Device Revocation

High-risk but common operation.

Flow:

```text
Devices
→ Device Detail
→ Revoke Device
→ Explain consequences
→ Confirm
→ Rust revoke
→ Update all devices
```

---

# 26. Revoke Explanation

Example:

```text
This device will lose access to your account and future messages.
```

---

# 27. Reauthentication

Require before high-risk actions where appropriate:

```text
revoke device
show recovery key
reset identity
export sensitive backup key
transfer ownership of account device trust
```

---

# 28. Android Reauthentication

Use:

```text
BiometricPrompt
device credential
```

through Kotlin platform layer.

---

# 29. Desktop Reauthentication

Use:

```text
OS keyring/credential prompt
application passphrase
```

depending platform/security architecture.

---

# 30. Rust Requests Authentication Semantically

Example:

```rust
pub enum ReauthPurpose {
    RevokeDevice(DeviceId),
    ShowRecoveryKey,
    RotateRecoveryKey,
    ResetIdentity,
    ExportRecoveryMaterial,
}
```

Platform performs actual prompt.

---

# 31. Reauth Result

Rust receives:

```text
success
cancel
failure
```

not biometric internals.

---

# 32. Revoke Current Device

Usually:

```text
Sign out / Remove this device
```

different wording from remote revoke.

---

# 33. Removing Current Device

May require:

```text
backup/recovery warning
```

if last trusted device.

---

# 34. Last Trusted Device

Critical edge case.

If user is about to remove/revoke last trusted device:

```text
require recovery confirmation
```

or prevent action until recovery configured.

---

# 35. Lost Device Flow

Security Center shortcut:

```text
I lost a device
```

Flow:

```text
select lost device
revoke
review sessions/events
optionally rotate affected credentials
```

---

# 36. Stolen Device Flow

Same plus stronger guidance:

```text
Revoke immediately
Review recent activity
Change recovery credential if exposed
```

---

# 37. Unknown Device

If user sees unfamiliar device:

```text
This device may have access to your account
Revoke
Review Security Events
```

---

# 38. Compromise Response

Dedicated guided flow:

```text
Revoke suspicious devices
Review identity state
Rotate recovery material if necessary
Verify backup
Review recent security events
```

---

# 39. Panic Button?

Avoid vague:

```text
Secure my account
```

unless exact actions are transparent.

Better guided checklist.

---

# 40. Security Event Model

```rust
pub struct SecurityEventView {
    pub id: SecurityEventId,
    pub kind: SecurityEventKind,
    pub severity: SecurityEventSeverity,
    pub occurred_at: Timestamp,
    pub resolved: bool,
    pub related_device: Option<DeviceId>,
    pub related_contact: Option<AccountId>,
    pub actions: Vec<SecurityEventAction>,
}
```

---

# 41. Security Event Kinds

```rust
pub enum SecurityEventKind {
    DeviceLinked,
    DeviceRevoked,
    DeviceLinkDenied,
    IdentityChanged,
    VerificationFailed,
    RecoveryConfigured,
    RecoveryChanged,
    BackupFailed,
    KeyRotation,
    SuspiciousAuthorization,
    SecurityPolicyChanged,
}
```

---

# 42. Severity

```rust
pub enum SecurityEventSeverity {
    Info,
    Warning,
    Critical,
}
```

---

# 43. Security Events Screen

Sort:

```text
newest first
```

Filters:

```text
All
Warnings
Critical
Devices
Identity
Recovery
```

---

# 44. Event Row

Show:

```text
title
time
severity
resolved/unresolved
related device/contact
```

---

# 45. Event Detail

Show:

```text
what happened
what it means
what action is recommended
what is already resolved
```

---

# 46. Avoid Raw Crypto Errors

Do not show:

```text
signature verification error 0x72
```

except developer diagnostics.

---

# 47. Security Event Resolution

Rust owns resolved state.

UI can perform actions then event becomes resolved.

---

# 48. Identity & Verification Section

Shows user's own identity health and contact-verification summary.

---

# 49. Own Identity

Display:

```text
account identity
short fingerprint
created date
verification/recovery state
```

---

# 50. Full Fingerprint

Advanced detail.

Copy/share only explicitly.

---

# 51. Identity Key Health

Recommended semantic states:

```rust
pub enum IdentityKeyHealth {
    Healthy,
    RotationRecommended,
    RecoveryRequired,
    Compromised,
}
```

---

# 52. Key Rotation

Normal users should see:

```text
Security keys updated
```

not key epochs.

---

# 53. Automatic Key Rotation

If transparent:

```text
no interruptive UI
```

unless action required.

---

# 54. Manual Key Rotation

Advanced/security operation.

Needs explicit explanation.

---

# 55. Identity Reset

Very high-risk.

Could invalidate:

```text
contact verification
device trust
recovery
```

depending architecture.

---

# 56. Identity Reset UX

Require:

```text
clear consequences
reauthentication
typed/secondary confirmation
```

---

# 57. Do Not Put Identity Reset Near Normal Settings

Place under:

```text
Advanced / Dangerous Actions
```

---

# 58. Contact Verification Summary

Security Center can show:

```text
3 contacts need verification review
```

linking to Part 08.

---

# 59. Identity Changed Contacts

High-priority list:

```text
Alice — identity changed
Bob — verification expired/invalid
```

---

# 60. Recovery Section

Purpose:

```text
regain account/device access after losing devices
```

---

# 61. Recovery Methods

Possible architecture:

```text
Recovery key
Recovery passphrase
Trusted device approval
Encrypted backup
Trusted contact? only if designed
```

UI shows only actually supported methods.

---

# 62. Recovery Status

```rust
pub enum RecoveryStatus {
    NotConfigured,
    Configured,
    NeedsReview,
    Invalid,
}
```

---

# 63. Recovery Overview

Show:

```text
Recovery configured
Last verified: 3 months ago
```

or:

```text
No recovery method configured
Set up recovery
```

---

# 64. Recovery Key

If product uses a recovery key:

```text
high-entropy secret
```

not a friendly password.

---

# 65. Recovery Key Display

Sensitive screen.

Require reauth.

Show:

```text
Copy
Save securely
Print
Verify
```

---

# 66. Recovery Key Never Auto-Copied

Explicit user action only.

---

# 67. Recovery Key Screenshot

Product may warn:

```text
Anyone with this key may be able to recover your account.
```

---

# 68. Android Secure Window

For recovery-key screen, Kotlin may enable secure-window policy to block screenshots/recents preview if product chooses.

---

# 69. Desktop Sensitive Screen

Can blur/hide on app lock or window background where feasible.

---

# 70. Recovery Key Verification

After setup:

```text
re-enter selected groups
or
scan saved copy
```

to prove user stored it.

---

# 71. Recovery Key Rotation

High-risk.

Flow:

```text
reauth
generate new recovery material
show/save
verify
activate
invalidate old material
```

---

# 72. Atomic Recovery Rotation

Old recovery must remain valid until new method is fully confirmed, unless security policy requires immediate cutover.

---

# 73. Recovery Failure Avoidance

Never leave user with:

```text
old key invalid
new key not saved
```

---

# 74. Recovery Drill

Optional security feature:

```text
Test Recovery
```

without actually logging out.

---

# 75. Recovery Drill Purpose

Verify:

```text
user still has recovery material
backup can be opened
```

---

# 76. Recovery Drill Privacy

Do not upload recovery key.

Local validation where possible.

---

# 77. Recovery Passphrase

If supported:

```text
user-chosen secret
```

must have clear strength guidance.

---

# 78. Do Not Pretend Passphrase Is Equivalent to Recovery Key

If security differs, explain.

---

# 79. Backup Recovery Integration

Part 33 backend + Part 16 UI later.

Security Center should show:

```text
Last backup
Backup encrypted
Recovery method
Verification status
```

---

# 80. Backup Security States

```rust
pub enum BackupSecurityState {
    Healthy,
    Stale,
    Missing,
    Unverified,
    Failed,
}
```

---

# 81. Backup Key

If backup has independent key:

```text
do not conflate with account recovery key
```

unless architecture intentionally unifies them.

---

# 82. Backup Key UX

Label precisely:

```text
Backup Recovery Key
```

if distinct.

---

# 83. Recovery vs Backup

Important distinction:

```text
Account recovery
≠
Backup decryption
```

unless system explicitly makes them one.

---

# 84. Security Center Should Explain Both

Example:

```text
Account Recovery: configured
Backup Recovery: configured
```

---

# 85. Lost All Devices Flow

Dedicated recovery entry.

Possible:

```text
Restore account
Scan/import backup
Enter recovery key
Create fresh device identity
Verify recovered state
```

---

# 86. Fresh Device Identity After Recovery

Do not restore stale device session identity blindly.

---

# 87. Recovery Screen Trust Boundary

User enters recovery material into local trusted UI only.

---

# 88. Clipboard Hygiene

After copying recovery key:

```text
offer Clear Clipboard
```

where platform supports.

Do not silently monitor clipboard afterward.

---

# 89. Recovery QR

If supported for transfer:

```text
short-lived
```

and high-risk.

---

# 90. Recovery QR Warning

Strong:

```text
Do not share this code.
```

---

# 91. Security Center — Android Layout

Recommended:

```text
TopAppBar
Security

Status card

Devices
Identity
Recovery
Backups
Security Events
Privacy

Advanced
```

---

# 92. Security Center — Desktop Layout

Recommended two-pane:

```text
Left:
    Overview
    Devices
    Identity
    Recovery
    Backups
    Events
    Privacy
    Advanced

Right:
    selected content
```

---

# 93. Tablet/Foldable

List/detail layout.

---

# 94. Device Detail Android

Full screen or bottom sheet for simple details.

High-risk actions use full-screen/dialog confirmation.

---

# 95. Device Detail Desktop

Inspector/detail pane.

---

# 96. Security Status Banner

Global app shell may show only unresolved high-priority issues.

Do not permanently show "secure" banners.

---

# 97. Critical Security Banner

Examples:

```text
Unknown device linked
Identity changed
Recovery invalid
```

Actions:

```text
Review
```

---

# 98. Dismissal

Critical security issue should not become resolved merely because banner dismissed.

---

# 99. Snooze

Optional for non-critical warnings.

---

# 100. Unresolved State

Rust owns.

---

# 101. Security Event Notifications

Part 13.

Tap:

```text
SecurityEvent(SecurityEventId)
```

---

# 102. Device Link Notification

After new device:

```text
New device linked
```

with review action.

---

# 103. Revocation Notification

Other devices receive:

```text
Device revoked
```

informational.

---

# 104. Recovery Change Notification

Other trusted devices may receive:

```text
Recovery settings changed
```

---

# 105. Identity Reset Notification

Critical.

---

# 106. Security Center and Contact Verification

Contact identity changes link to Part 08.

---

# 107. Security Center and Device Linking

New-device action links to Part 12.

---

# 108. Security Center and Backup

Backup actions link to Part 16 UI.

---

# 109. Security Center and Settings

Privacy/presence links to Parts 14/18.

---

# 110. Privacy Section

Can summarize high-impact controls:

```text
Notification previews
Read receipts
Typing
Presence
Blocked contacts
```

Detailed settings remain elsewhere.

---

# 111. App Lock

Optional:

```text
Require biometric/device credential to open app
```

---

# 112. App Lock Scope

Protects UI access.

Does not necessarily stop:

```text
background receive
calls
sync
```

---

# 113. App Lock Timeout

Potential:

```text
Immediately
1 minute
5 minutes
30 minutes
```

---

# 114. App Lock on Desktop

Could use:

```text
application passphrase
OS authentication
```

---

# 115. Sensitive Action Reauth Separate from App Lock

Even if app unlocked, high-risk action may require reauth.

---

# 116. Screen Privacy

Optional:

```text
hide content in Android recent-apps snapshot
```

while locked.

---

# 117. Clipboard Security

Sensitive secrets copied only on explicit action.

---

# 118. Key Export

Do not expose raw identity private key export in normal UX.

---

# 119. Advanced Key Material

If product allows export:

```text
Advanced
high-risk
reauthenticated
strong warning
```

---

# 120. Device Enrollment History

Security events provide history.

Do not show stale "trusted" device after revocation.

---

# 121. Device Revocation Propagation

UI can show:

```text
Revoking…
```

then:

```text
Revoked
```

if operation distributed/async.

---

# 122. Revocation Pending

```rust
pub enum RevocationState {
    Active,
    Pending,
    Complete,
    Failed,
}
```

---

# 123. Offline Revocation

High-risk operation may require connectivity to authoritative peers/server.

If architecture supports offline signed revocation:

```text
show pending propagation
```

---

# 124. Revocation Failure

Show:

```text
Could not revoke device
Retry
```

Do not pretend success.

---

# 125. Self-Revocation

Current device removal can wipe local secrets after successful account-state update.

---

# 126. Local Wipe

If user chooses:

```text
Remove account data from this device
```

requires separate explicit action.

---

# 127. Remote Wipe

Do not promise remote wipe unless platform/backend actually supports it.

---

# 128. Lost Device Messaging

Say:

```text
Revoke access
```

not:

```text
Wipe device
```

unless true.

---

# 129. Key Health Events

Examples:

```text
device key rotated
recovery key changed
identity key changed
backup key invalid
```

---

# 130. Routine Key Rotation

Informational, not alarming.

---

# 131. Unexpected Root Identity Change

Critical.

---

# 132. Key Expiry

If product uses expiring certs/keys:

```text
renew automatically
```

and only surface if action required.

---

# 133. Security Details vs Diagnostics

Normal Security Center:

```text
human meaning
```

Diagnostics:

```text
key IDs
epoch IDs
signature status
transport security details
```

---

# 134. No Raw Private Keys in Diagnostics

Hard rule.

---

# 135. Device Capability View

```rust
pub struct DeviceSecurityCapabilities {
    pub can_revoke: bool,
    pub can_rename: bool,
    pub can_view_activity: bool,
    pub can_reauthenticate: bool,
}
```

---

# 136. Recovery Capability View

```rust
pub struct RecoveryCapabilities {
    pub can_create: bool,
    pub can_rotate: bool,
    pub can_test: bool,
    pub can_export: bool,
}
```

---

# 137. Security Overview Snapshot

```rust
pub struct SecurityCenterSnapshot {
    pub health: SecurityHealth,
    pub trusted_device_count: u32,
    pub unresolved_event_count: u32,
    pub recovery_status: RecoveryStatus,
    pub backup_security: BackupSecurityState,
    pub top_events: Vec<SecurityEventView>,
}
```

---

# 138. Security Presentation API

```rust
pub trait SecurityPresentation {
    async fn snapshot(
        &self,
    ) -> Result<SecurityCenterSnapshot, UiError>;

    async fn events(
        &self,
        cursor: Option<SecurityEventCursor>,
    ) -> Result<SecurityEventPage, UiError>;

    async fn resolve_event(
        &self,
        event: SecurityEventId,
    ) -> Result<(), UiError>;
}
```

---

# 139. Device Security API

```rust
pub trait DeviceSecurityPresentation {
    async fn devices(
        &self,
    ) -> Result<Vec<DeviceSecurityView>, UiError>;

    async fn device(
        &self,
        id: DeviceId,
    ) -> Result<DeviceSecurityView, UiError>;

    async fn rename(
        &self,
        id: DeviceId,
        name: String,
    ) -> Result<(), UiError>;

    async fn revoke(
        &self,
        id: DeviceId,
        auth: ReauthProof,
    ) -> Result<RevocationResultView, UiError>;
}
```

---

# 140. Recovery Presentation API

```rust
pub trait RecoveryPresentation {
    async fn status(
        &self,
    ) -> Result<RecoveryStatusView, UiError>;

    async fn create(
        &self,
        auth: ReauthProof,
    ) -> Result<RecoveryMaterialView, UiError>;

    async fn verify(
        &self,
        proof: RecoveryVerificationInput,
    ) -> Result<RecoveryVerificationResult, UiError>;

    async fn rotate(
        &self,
        auth: ReauthProof,
    ) -> Result<RecoveryMaterialView, UiError>;
}
```

---

# 141. Reauthentication API Boundary

Rust can request:

```rust
pub struct ReauthChallenge {
    pub id: ReauthChallengeId,
    pub purpose: ReauthPurpose,
}
```

Platform returns a short-lived proof/token understood by security service.

---

# 142. Platform Reauth Adapter — Android

Uses:

```text
BiometricPrompt
device credential
```

---

# 143. Platform Reauth Adapter — Desktop

Uses platform/local secure credential mechanism.

---

# 144. Security Events

```rust
pub enum SecurityUiEvent {
    HealthChanged(SecurityHealth),
    DeviceChanged(DeviceSecurityView),
    DeviceRemoved(DeviceId),
    EventAdded(SecurityEventView),
    EventUpdated(SecurityEventView),
    RecoveryChanged(RecoveryStatusView),
    BackupSecurityChanged(BackupSecurityState),
}
```

---

# 145. Android ViewModel

Owns:

```text
selected section
dialogs
reauth effects
secure-screen effects
copy/share effects
```

Rust owns security truth.

---

# 146. Dioxus Presenter

Owns:

```text
sidebar selection
detail panes
modal state
clipboard/export effects
```

---

# 147. No Security Decisions in UI

Hard rule.

---

# 148. Sensitive UI Effect

Examples:

```text
RequestReauth
EnableSecureWindow
CopySensitiveSecret
OpenSystemSecuritySettings
```

---

# 149. Recovery Material UI Model

Avoid exposing raw secret broadly.

Use a dedicated short-lived sensitive model.

---

# 150. Sensitive Model Lifetime

UI should release/clear after:

```text
screen exit
timeout
app lock
background if policy
```

---

# 151. Recovery Secret in Compose State

Avoid keeping long-lived secret in ordinary `StateFlow`.

Use scoped sensitive state.

---

# 152. Recovery Secret in Dioxus

Same principle.

---

# 153. Clipboard Timeout

If platform supports controlled clipboard clearing:

```text
offer clear after 1 minute
```

but do not promise guaranteed deletion from external clipboard managers.

---

# 154. Export Recovery Material

If supported:

```text
encrypted file only
```

or safe text/print flow.

---

# 155. Print Recovery Key

Desktop optional.

Warn about physical security.

---

# 156. Screenshot Testing

Never include real recovery material in screenshot tests.

Use fake fixtures.

---

# 157. Accessibility — Security Overview

Screen reader:

```text
Security needs attention. One unresolved critical event.
```

---

# 158. Device Row Accessibility

```text
Pixel 10, trusted, active now, this device
```

---

# 159. Recovery Accessibility

Recovery key should be readable in grouped chunks.

---

# 160. Sensitive Secret Reading

Provide:

```text
Show
Hide
Copy
```

Controls.

---

# 161. Default Hidden Secret

Recovery secret may initially be obscured after creation until user taps Show.

---

# 162. Color Independence

Critical/warning/healthy always have:

```text
text
icon
```

---

# 163. Large Font

Security explanations must wrap.

Do not truncate consequences.

---

# 164. RTL

Security UI mirrors.

Cryptographic fingerprints/keys retain canonical direction.

---

# 165. Reduced Motion

Security state changes do not need animation.

---

# 166. Keyboard Desktop

Support:

```text
arrow navigation
Enter
Shift+F10
Ctrl/Cmd+F in event list
```

---

# 167. Android TalkBack

All high-risk action confirmations fully accessible.

---

# 168. Confirmation UX

For high-risk action, confirmation states:

```text
What will happen
What will not happen
Whether it can be undone
```

---

# 169. Destructive Action Examples

```text
Revoke Device
Reset Identity
Rotate Recovery Key
Delete Recovery Method
```

---

# 170. Do Not Require Typing Device Name Unless Needed

Confirmation friction should match actual risk.

---

# 171. Extremely High-Risk Action

Identity reset can require:

```text
reauth
secondary confirmation
typed phrase
```

---

# 172. Security Center Empty State

Security event list:

```text
No recent security events
```

---

# 173. Recovery Not Configured

Actionable card:

```text
Set Up Recovery
```

---

# 174. Backup Missing

```text
No verified backup
Create Backup
```

link to Part 16.

---

# 175. Current Device Only

If only one device:

```text
1 trusted device
```

and recovery recommendation becomes more important.

---

# 176. Device Offline

Last active may be stale.

Do not label:

```text
offline
```

unless proven.

---

# 177. Device Unknown

If metadata incomplete:

```text
Unknown device
```

with DeviceId detail.

---

# 178. Lost Device Revocation Confirmation

Example:

```text
Revoke Pixel 10?
It will no longer be able to access your account or receive future messages.
```

---

# 179. Existing Sessions

If architecture has session tokens:

```text
revocation invalidates them
```

UI can say:

```text
This signs the device out.
```

only if true.

---

# 180. Remote Historical Data

Revocation cannot erase copies already exported/decrypted on another device.

Important honesty.

---

# 181. Revocation Explanation

Do not claim:

```text
all data is erased remotely
```

unless guaranteed.

---

# 182. Recovery Honesty

Explain what recovery can restore:

```text
account identity
encrypted backup
contacts
history
```

depending actual architecture.

---

# 183. Recovery Cannot Restore Missing Data

If no backup/archive exists:

```text
recovery may restore account access but not missing message history
```

---

# 184. Compromise Checklist

Recommended:

```text
1. Revoke unknown devices
2. Review recent security events
3. Verify recovery method
4. Re-verify affected contacts if identity changed
5. Create fresh backup
```

---

# 185. Security Guided Flow

UI may present as step-by-step checklist.

Rust reports completion.

---

# 186. Security Event Correlation

Multiple events can be grouped:

```text
New device linked
Recovery changed 2 min later
```

advanced future feature.

---

# 187. Alerts vs Events

Notification is transient alert.

Security event is durable audit record.

---

# 188. Security Event Retention

Longer than normal notifications.

---

# 189. Search Security Events

Part 11 may search titles/metadata locally.

Do not index secret contents.

---

# 190. Backup Security Integration

Part 16 should consume the same:

```text
RecoveryStatus
BackupSecurityState
```

not duplicate.

---

# 191. Device Linking Integration

Part 12 success emits:

```text
DeviceLinked
```

security event.

---

# 192. Contact Verification Integration

Part 08 identity mismatch emits:

```text
VerificationFailed
IdentityChanged
```

event.

---

# 193. Call Security Integration

Part 07 security failure can link to Security Center.

---

# 194. Notification Integration

Part 13 Security category opens exact event.

---

# 195. Privacy Settings Integration

Part 18 can deep-link back to Security Center for sensitive controls.

---

# 196. Multi-Device Security Sync

Device/recovery/security event state must synchronize securely across trusted devices.

---

# 197. Current Device Race

If current device is revoked from elsewhere:

```text
immediately transition to revoked/session-expired UI
```

---

# 198. Revoked Current Device UX

Show:

```text
This device no longer has access to the account.
```

Actions:

```text
Link Again
Recover Account
Remove Local Data
```

according to state.

---

# 199. Pending Revocation While Offline

When device reconnects and learns revoked state:

```text
lock account access
```

before normal sync.

---

# 200. Recovery Race

If recovery method rotates on another device:

```text
old recovery UI becomes stale
```

Rust rejects stale verify/export actions.

---

# 201. Stale Security Screen

Before high-risk commit:

```text
Rust revalidates current security state
```

---

# 202. No Optimistic High-Risk Security Actions

Do not optimistically show:

```text
Revoked
Recovery changed
```

before Rust confirmation.

---

# 203. Safe Optimism

Renaming a device can be optimistic if rollback supported.

---

# 204. Security Error Taxonomy

```rust
pub enum SecurityUiErrorKind {
    ReauthRequired,
    ReauthFailed,
    DeviceAlreadyRevoked,
    LastTrustedDevice,
    RecoveryNotConfigured,
    RecoveryVerificationFailed,
    StaleSecurityState,
    PolicyDenied,
    OfflineUnavailable,
    Internal,
}
```

---

# 205. Error UX

Reauth required:

```text
Authenticate to continue
```

Last trusted device:

```text
Set up recovery before removing your last trusted device
```

Recovery verification failed:

```text
Recovery key did not match
```

---

# 206. Do Not Reveal Secret Comparison Details

Failure should not leak cryptographic oracle information.

---

# 207. Testing Matrix

Required:

```text
healthy overview
attention overview
critical event
trusted device list
new device
unknown device
revoke remote device
remove current device
last trusted device
lost device
recovery setup
recovery verify
recovery rotate
backup stale
identity change
security event resolution
```

---

# 208. Android Tests

Verify:

```text
BiometricPrompt
device credential fallback
secure window
process death
background/foreground
TalkBack
large font
clipboard handling
```

---

# 209. Desktop Tests

Verify:

```text
two-pane security center
keyboard
OS credential prompt
clipboard
multi-window state
device revoke
```

---

# 210. Multi-Device Tests

Revoke phone from desktop.

Phone becomes unauthorized.

Add new device from phone.

Desktop sees event.

---

# 211. Recovery Rotation Test

Old key remains valid until new key verified, according to policy.

---

# 212. Recovery Loss Test

User cancels before confirmation.

Old recovery remains intact.

---

# 213. Security Event Persistence Test

Warning survives restart until resolved.

---

# 214. Current Device Revoked Test

UI immediately exits normal account surfaces.

---

# 215. Offline Revocation Test

If supported, pending state is clear.

If unsupported, action fails honestly.

---

# 216. Accessibility Test

Complete recovery setup and device revocation without relying on color/gesture.

---

# 217. Secret Leakage Test

No recovery key/fingerprint/private material appears in logs/crash reports/analytics.

---

# 218. Initial Production Scope

Ship:

```text
security overview
trusted devices
device details
new-device events
device revocation
lost-device flow
identity health
contact verification warnings
recovery setup
recovery key verification
recovery key rotation
backup security summary
security event log
reauthentication for sensitive actions
Android secure screen for recovery material
```

Defer:

```text
social recovery
hardware security-key enrollment
advanced enterprise PKI
complex security-score systems
remote wipe claims
automatic forensic correlation
```

unless backend explicitly supports them.

---

# 219. Definition of Done

UI/UX Part 15 is complete when:

- Security Center has Overview, Devices, Identity, Recovery, Backups, Events, Privacy, and Advanced sections
- Rust owns device trust, revocation, identity/key health, recovery validation, and event resolution
- current/pending/revoked/compromised device states are distinct
- own-device linking integrates with Part 12 but authorization truth remains in Rust
- high-risk operations require semantic reauthentication
- Android uses BiometricPrompt/device credential through platform adapter
- lost/stolen/unknown-device flows are explicit
- last-trusted-device removal is guarded by recovery policy
- revocation does not falsely promise remote data erasure
- identity reset is isolated as a dangerous action
- recovery setup, display, verification, rotation, and cancellation are safe and atomic
- account recovery and backup recovery are not conflated unless architecture intentionally unifies them
- recovery secrets are short-lived sensitive UI state and excluded from normal logs/state flows
- security events are durable audit records separate from notifications
- critical unresolved events persist until actually resolved
- multi-device revocation/recovery/security synchronization is defined
- accessibility, RTL, large font, secure-screen, clipboard, and reauth behavior are explicit
- Rust security, device, recovery, reauth, and event presentation APIs are specified
- compromise, offline, race, current-device-revocation, recovery-loss, and leakage tests are included

---

# 220. Final Architecture

```text
                    RUST SECURITY CORE
                           │
       ┌───────────────────┼───────────────────┐
       │                   │                   │
    Devices             Identity           Recovery
       │                   │                   │
   Authorization         Key Health       Recovery Material
   Revocation            Verification     Backup Security
       │                   │                   │
       └───────────────────┼───────────────────┘
                           │
                 Security Presentation
                    ┌──────┴──────┐
                    │             │
                 Dioxus        Compose
                    │             │
             Desktop Security  Android Security
```

High-risk platform path:

```text
Rust requests reauth
      │
      ▼
Platform credential / biometric
      │
      ▼
short-lived proof
      │
      ▼
Rust performs security action
```

---

# 221. Final Principle

The Security Center should help users take correct action without teaching them protocol internals.

The right model is:

```text
clear device trust
+
clear recovery readiness
+
clear actionable security events
+
explicit high-risk confirmation
+
Rust-authoritative security state
```

not:

```text
show lots of cryptographic details and expect users to infer what matters
```

This gives Dioxus desktop and Android Compose a security experience that is understandable, recoverable, and resistant to UI-level mistakes while the Rust core remains authoritative for all trust and key decisions.
