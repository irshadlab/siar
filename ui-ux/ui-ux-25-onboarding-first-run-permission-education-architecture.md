# UI/UX Part 25 — Onboarding, First Run & Permission Education UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 25  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete onboarding, first-run, account-entry, existing-device linking, recovery, profile setup, permission education, capability readiness, privacy defaults, skipped-step recovery, offline-first setup, accessibility, and Rust presentation architecture across desktop and Android.

---

# 1. Purpose

First-run UX determines whether users understand:

```text
what the product is
whether they need an account
how identity works
how to link another device
how to recover an existing account
what permissions are actually needed
which permissions are optional
what still works if they decline
```

Poor onboarding often causes users to grant permissions they do not understand or abandon setup before reaching the useful product.

The governing principle is:

> **Ask for the minimum information and permissions required for the next meaningful task, and explain consequences before the operating system prompt appears.**

---

# 2. Architectural Position

```text
Rust Identity / Account / Capability Core
        │
        ├── account state
        ├── device state
        ├── recovery readiness
        ├── onboarding progress
        ├── required capabilities
        ├── optional capabilities
        └── feature readiness
        │
        ▼
Onboarding Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

Platform adapters own:

```text
camera permission
microphone permission
notification permission
nearby/Bluetooth permission
location permission
OS credential/biometric
file picker
system settings navigation
```

Rust owns the semantic meaning of why a capability is needed.

---

# 3. Main First-Run Entry

Recommended first screen:

```text
Welcome

[Create New Account]
[Use Existing Account]
```

Optional secondary:

```text
Learn how this works
```

---

# 4. Avoid Immediate Permission Prompts

Do not launch first-run with:

```text
notifications
camera
microphone
nearby devices
location
```

all at once.

---

# 5. Account Entry Modes

```rust
pub enum OnboardingEntryMode {
    CreateNewAccount,
    LinkExistingAccount,
    RecoverExistingAccount,
}
```

---

# 6. Create New Account

Purpose:

```text
create new account identity
authorize current device
configure minimal profile
```

---

# 7. Link Existing Account

Purpose:

```text
authorize this device from an already trusted device
```

Uses Part 12 device-link architecture.

---

# 8. Recover Existing Account

Purpose:

```text
regain access using supported recovery material
```

Uses Parts 15/16.

---

# 9. Do Not Conflate Link and Recover

Hard rule.

Linking assumes:

```text
at least one trusted device is available
```

Recovery assumes:

```text
trusted device may be unavailable
```

---

# 10. First-Run State

```rust
pub enum FirstRunState {
    Uninitialized,
    ChoosingEntryMode,
    CreatingIdentity,
    LinkingDevice,
    Recovering,
    ConfiguringProfile,
    ReviewingPrivacy,
    CapabilitySetup,
    Ready,
}
```

---

# 11. Onboarding Progress

Progress should be semantic, not artificial percentage.

Example:

```text
Account
Profile
Permissions
Ready
```

---

# 12. Avoid Long Wizard Feel

Prefer:

```text
3–5 meaningful steps
```

with optional configuration deferred.

---

# 13. Create New Account Flow

Recommended:

```text
Welcome
→ Create New Account
→ Identity created locally
→ Name/profile
→ Recovery recommendation
→ Optional permission setup
→ Home
```

---

# 14. Local-First Identity Creation

Should work without Internet where architecture allows.

---

# 15. Offline Creation UX

Example:

```text
Account created on this device
You can start using nearby/local features now.
```

---

# 16. Server Registration

If some account service requires network later:

```text
complete asynchronously
```

and clearly show pending status.

---

# 17. Profile Setup

Minimum:

```text
Display name
Optional avatar
```

---

# 18. Display Name

Explain:

```text
This is how people will recognize you.
```

---

# 19. Name Validation

Rust-owned.

---

# 20. Avatar

Optional.

Do not block onboarding.

---

# 21. Avatar Permissions

Use system picker.

No storage-wide permission.

---

# 22. Profile Skip

Allow:

```text
Skip for now
```

where product can generate safe fallback.

---

# 23. Recovery Recommendation

After creating account:

```text
Set Up Recovery
```

strongly recommended but not necessarily mandatory.

---

# 24. Recovery Setup

Links into Part 15.

---

# 25. Recovery Skip

If allowed:

```text
Not now
```

but show honest consequence:

```text
If you lose all trusted devices, you may not be able to recover your account.
```

---

# 26. Link Existing Account Flow

New device:

```text
Use Existing Account
→ Link from Trusted Device
→ Scan QR / Enter Code
→ Authenticate
→ Existing device approves
→ Optional SAS
→ Device linked
→ Sync/setup
→ Home
```

---

# 27. Manual Link Fallback

Always available if QR/camera inaccessible:

```text
Enter Link Code
```

---

# 28. Desktop Link Flow

Recommended:

```text
Enter code
Paste code
Optional webcam scan
```

---

# 29. Android Link Flow

Recommended:

```text
Scan QR
Enter code
NFC optional
```

---

# 30. Linking Consequence

Before approval:

```text
This device will be able to access your account and receive messages.
```

---

# 31. New Device Name

After linking:

```text
Name this device
```

optional.

---

# 32. Device Sync Setup

Show semantic phases:

```text
Setting up account
Syncing conversations
Preparing search
Ready
```

---

# 33. Do Not Block on Derived Work

Search indexing/thumbnails can continue after Home is usable.

---

# 34. Recovery Flow

Recommended:

```text
Use Existing Account
→ Recover Account
→ Choose Recovery Method
→ Verify Recovery Material
→ Create Fresh Device Identity
→ Restore Account Metadata
→ Optional Restore Backup
→ Home
```

---

# 35. Recovery Key Entry

Sensitive screen.

Part 15 rules apply.

---

# 36. Backup Restore Choice

If recoverable backup found:

```text
Restore Message History
Skip for now
```

---

# 37. Account Recovery vs Data Restore

Explain:

```text
Account access restored
Message history can be restored separately
```

if architecture distinguishes.

---

# 38. Permission Philosophy

Permissions should be requested:

```text
just in time
```

when the user performs an action needing them.

---

# 39. Permission Education Layer

Before OS prompt, show:

```text
what feature needs it
why
what data is accessed
whether optional
what works without it
```

---

# 40. Permission Model

```rust
pub struct PermissionEducationView {
    pub capability: AppCapability,
    pub title: String,
    pub reason: String,
    pub required_for: Vec<FeatureKind>,
    pub optional: bool,
    pub alternative: Option<CapabilityAlternative>,
}
```

---

# 41. App Capabilities

```rust
pub enum AppCapability {
    Notifications,
    Camera,
    Microphone,
    NearbyDevices,
    Location,
    Files,
    Biometric,
    ScreenCapture,
}
```

---

# 42. Android Notification Permission

Request when user meaningfully wants:

```text
message alerts
incoming call alerts
```

---

# 43. Notification Education

Example:

```text
Allow notifications to receive message and call alerts while the app is closed.
```

---

# 44. Notification Decline

App remains usable.

---

# 45. Notification Decline Consequence

Show:

```text
You may miss alerts while the app is not open.
```

---

# 46. Camera Permission

Request when user chooses:

```text
Scan QR
Take Photo
Video Call
```

not during generic onboarding.

---

# 47. Camera Alternative

For device linking:

```text
Enter code manually
```

---

# 48. Microphone Permission

Request when user chooses:

```text
Voice Note
Audio Call
Video Call
```

---

# 49. Microphone Decline

Text messaging still works.

---

# 50. Nearby Devices Permission

Request when user chooses:

```text
Find Nearby
Offline Nearby Messaging
```

---

# 51. Nearby Education

Example:

```text
Nearby access lets this app discover devices close to you for local communication.
```

---

# 52. Legacy Location Requirement

On Android versions where nearby scanning technically requires location-related permission:

```text
explain accurately
```

without claiming precise location is shared if it is not.

---

# 53. Location Permission

Do not request for general nearby discovery unless platform requires it.

Use for:

```text
SOS location
explicit location sharing
```

---

# 54. Location Education

Example:

```text
Share your location with selected emergency contacts during an SOS.
```

---

# 55. Location Decline

SOS can still send without location.

---

# 56. File Access

Use:

```text
system picker
SAF
native file dialog
```

rather than broad filesystem permission.

---

# 57. Biometric

Request only for:

```text
app lock
high-risk reauthentication
recovery key access
```

---

# 58. Screen Capture Permission

Request when:

```text
screen sharing
```

starts.

---

# 59. Permission Timing Rule

Never request a permission:

```text
because the app may need it someday
```

---

# 60. Pre-Permission Screen

Structure:

```text
Feature title
Why access is needed
What happens if denied
[Continue]
[Not Now]
```

---

# 61. OS Prompt

Appears only after user taps:

```text
Continue
```

---

# 62. Denied Once

Offer alternative.

---

# 63. Permanently Denied / Don't Ask Again

Show:

```text
Open Settings
```

---

# 64. Permission State

```rust
pub enum CapabilityPermissionState {
    NotRequested,
    Granted,
    Denied,
    BlockedBySystem,
    Unavailable,
}
```

---

# 65. Capability Readiness

```rust
pub struct CapabilityReadinessView {
    pub capability: AppCapability,
    pub state: CapabilityPermissionState,
    pub feature_ready: bool,
}
```

---

# 66. Permission Is Not Capability

Example:

```text
camera permission granted
```

but:

```text
camera hardware unavailable
```

So readiness considers both.

---

# 67. Desktop Permission UX

Desktop OS behavior varies.

The product should normalize:

```text
camera
microphone
screen capture
notifications
```

where possible.

---

# 68. Desktop Permission Denial

Show system-specific action:

```text
Open System Settings
```

if available.

---

# 69. No Fake Desktop Permission Prompt

If OS does not expose runtime prompt, do not simulate one as authority.

---

# 70. First-Run Privacy Defaults

Recommended conservative defaults:

```text
read receipts: configurable
typing: configurable
presence: minimal
notification preview: sender-only or generic
unknown request privacy: strict
auto-download unknown requests: off
```

Exact defaults are product policy.

---

# 71. Privacy Review Step

Optional concise summary:

```text
Presence
Read Receipts
Notification Previews
```

with:

```text
Use Recommended Defaults
Customize
```

---

# 72. Avoid 20 Privacy Toggles During First Run

Detailed controls remain in Part 18 Settings.

---

# 73. Recommended Defaults Explanation

Use:

```text
You can change these anytime.
```

---

# 74. Contact Import / Discovery

Do not force.

---

# 75. System Contacts Permission

If future feature supports contact matching:

```text
request only when user chooses Find People From Contacts
```

---

# 76. Contact Import Privacy

Explain:

```text
which contact data leaves device, if any
```

---

# 77. Default Recommendation

Do not upload address book by default.

---

# 78. First Useful Action

Onboarding should end with one useful next step.

Examples:

```text
Add Contact
Link Another Device
Start Nearby
Open Chats
```

---

# 79. Ready Screen

Example:

```text
You're ready
[Add Contact]
[Go to Chats]
```

---

# 80. Avoid Celebration Overload

A short completion state is enough.

---

# 81. Post-Onboarding Checklist

Optional home card:

```text
Set up recovery
Enable notifications
Add an emergency contact
Link another device
```

---

# 82. Checklist Must Be Dismissible

Do not nag indefinitely.

---

# 83. Readiness Card

Use semantic states:

```text
Complete
Optional
Needs Attention
```

---

# 84. Onboarding Skip

Allow skipping optional steps.

---

# 85. Resume Later

Skipped setup surfaces remain discoverable in:

```text
Settings
Security Center
Devices
Emergency
```

---

# 86. Persisting Progress

Rust owns which durable onboarding tasks are complete.

---

# 87. UI Presentation Progress

UI can own:

```text
current page
animation
temporary text field
```

---

# 88. Onboarding Progress Model

```rust
pub struct OnboardingProgressView {
    pub account_ready: bool,
    pub profile_ready: bool,
    pub recovery_ready: bool,
    pub notification_ready: bool,
    pub nearby_ready: bool,
    pub emergency_ready: bool,
}
```

---

# 89. Do Not Gate Home on Optional Readiness

Example:

```text
notifications denied
```

must not block app.

---

# 90. Required Gates

Only truly required states can block:

```text
no valid account identity
device authorization incomplete
critical storage initialization failure
```

---

# 91. Offline-First Onboarding

If Internet unavailable:

```text
create account locally
configure profile
use nearby/local features
```

where backend permits.

---

# 92. Offline Existing-Device Link

Can use local QR/LAN/Bluetooth if backend supports.

---

# 93. Offline Recovery

Only if recovery material/backup is locally available.

---

# 94. Offline Status

Example:

```text
No Internet
You can continue setup and use nearby features.
```

---

# 95. Do Not Block on Connectivity Unless Required

Hard rule.

---

# 96. Network-Required Step

If some external service required:

```text
Internet is required for this step.
```

Offer:

```text
Try Later
```

if optional.

---

# 97. Error Recovery

Onboarding errors should preserve user work.

---

# 98. Profile Save Failure

Keep entered name/avatar choice.

---

# 99. Device Link Failure

Offer:

```text
Try Again
Enter Code
Use Recovery Instead
```

where appropriate.

---

# 100. Recovery Failure

Do not lose recovery input prematurely if safe to preserve temporarily.

---

# 101. Permission Failure

Offer alternate route.

---

# 102. Onboarding Crash

Resume from Rust durable state.

---

# 103. Process Death Android

Compose rebuilds from:

```text
OnboardingSnapshot
```

not from assumed page sequence.

---

# 104. Desktop Restart

Same.

---

# 105. Idempotency

Creating identity/linking device must be command-idempotent.

---

# 106. Duplicate Button Tap

Cannot create multiple accounts/devices.

---

# 107. Back Navigation Android

Back should:

```text
dismiss transient prompt
go to previous step
```

without destroying completed account state.

---

# 108. Cancel Create Account

Before identity committed:

```text
safe cancel
```

After identity committed:

```text
return to setup/home
```

not create another identity blindly.

---

# 109. Cancel Device Link

Cancels link session.

---

# 110. Cancel Recovery

Leaves current device unlinked/unrecovered.

---

# 111. Desktop Escape

Same semantic cancellation behavior.

---

# 112. Welcome Screen Accessibility

Screen reader:

```text
Welcome
Create new account
Use existing account
```

---

# 113. Avoid Auto-Focus Trap

Focus first meaningful heading/action.

---

# 114. Keyboard Desktop

Entire onboarding can be completed using keyboard.

---

# 115. TalkBack Android

All first-run flows must be TalkBack-complete.

---

# 116. QR Accessibility

Manual code always present.

---

# 117. Recovery Accessibility

Recovery key input is grouped and pasteable.

---

# 118. Permission Accessibility

Education screen must explain:

```text
permission name
feature
alternative
```

---

# 119. Large Text

Onboarding reflows vertically.

No fixed-height wizard cards.

---

# 120. RTL

All onboarding layouts mirror.

Codes/fingerprints remain canonical.

---

# 121. Reduced Motion

Step transitions can be instant/minimal fade.

---

# 122. Color Independence

Completion/warning states use:

```text
icon
text
```

---

# 123. Cognitive Accessibility

Keep one primary decision per screen.

---

# 124. Avoid Jargon

Use:

```text
Link this device
```

not:

```text
authorize secondary endpoint
```

---

# 125. Explain Security Consequences

Example:

```text
A linked device can access your account and receive messages.
```

---

# 126. Explain Recovery Consequences

Example:

```text
Without recovery, losing all trusted devices may make your account unrecoverable.
```

---

# 127. Do Not Overteach Cryptography

Deep details remain optional.

---

# 128. Permission Education Copy Rule

Never claim:

```text
We need this permission for security
```

unless true.

---

# 129. Permission Alternatives

Each optional permission should define fallback.

---

# 130. Permission Alternatives Matrix

Examples:

```text
Camera denied → manual code
Microphone denied → text
Notifications denied → foreground use
Nearby denied → QR/manual/Internet
Location denied → SOS without location
```

---

# 131. Permission Request Frequency

Do not repeatedly nag after decline.

---

# 132. Re-Prompt Policy

Request again only when:

```text
user retries feature
user opens related settings
```

---

# 133. System Settings Redirect

Use only when permission is blocked and user explicitly wants feature.

---

# 134. Notification Permission Re-Prompt

Avoid repeated startup banner.

Use readiness/settings card.

---

# 135. Onboarding Analytics

Do not collect sensitive recovery/security material.

---

# 136. Safe Metrics

Possible:

```text
step completion
permission denial rate
time-to-first-message
setup failure class
```

without identity/content.

---

# 137. Do Not Profile Users by Accessibility/Privacy Choice

---

# 138. Crash Reports

Redact:

```text
invite codes
recovery keys
QR payloads
contact identifiers
```

---

# 139. Onboarding Presentation Snapshot

```rust
pub struct OnboardingSnapshot {
    pub state: FirstRunState,
    pub entry_options: Vec<OnboardingEntryMode>,
    pub progress: OnboardingProgressView,
    pub capabilities: Vec<CapabilityReadinessView>,
    pub recovery: RecoveryStatus,
}
```

---

# 140. Onboarding Presentation API

```rust
pub trait OnboardingPresentation {
    async fn snapshot(
        &self,
    ) -> Result<OnboardingSnapshot, UiError>;

    async fn choose_entry(
        &self,
        mode: OnboardingEntryMode,
    ) -> Result<OnboardingSnapshot, UiError>;

    async fn create_account(
        &self,
        command: CreateAccountCommand,
    ) -> Result<OnboardingSnapshot, UiError>;

    async fn update_profile(
        &self,
        update: ProfileSetupCommand,
    ) -> Result<OnboardingSnapshot, UiError>;

    async fn complete(
        &self,
    ) -> Result<OnboardingResultView, UiError>;
}
```

---

# 141. Capability Education API

```rust
pub trait CapabilityEducationPresentation {
    async fn education(
        &self,
        capability: AppCapability,
    ) -> Result<PermissionEducationView, UiError>;

    async fn record_platform_result(
        &self,
        capability: AppCapability,
        result: PlatformPermissionResult,
    ) -> Result<CapabilityReadinessView, UiError>;
}
```

---

# 142. Device Link Handoff

Onboarding calls Part 12 API.

Do not duplicate device-link protocol in onboarding module.

---

# 143. Recovery Handoff

Uses Part 15/16 API.

---

# 144. Permission Platform Effect

Example:

```rust
pub enum OnboardingPlatformEffect {
    RequestNotificationPermission,
    RequestCameraPermission,
    RequestMicrophonePermission,
    RequestNearbyPermission,
    RequestLocationPermission,
    OpenSystemSettings(AppCapability),
    OpenFilePicker,
    OpenQrScanner,
}
```

---

# 145. Android ViewModel

Owns:

```text
step navigation
permission effects
temporary profile fields
animation
OS prompt timing
```

Rust owns account/readiness truth.

---

# 146. Dioxus Presenter

Owns:

```text
step routing
webcam/manual-code UI
native settings/file effects
focus
```

---

# 147. Capability Events

```rust
pub enum CapabilityUiEvent {
    CapabilityChanged(CapabilityReadinessView),
    PermissionPolicyChanged(AppCapability),
}
```

---

# 148. Onboarding Events

```rust
pub enum OnboardingUiEvent {
    StateChanged(FirstRunState),
    ProgressChanged(OnboardingProgressView),
    Ready,
}
```

---

# 149. App Entry Gate

At startup:

```text
if account/device ready
    → normal shell
else
    → onboarding
```

---

# 150. Returning User

No onboarding replay unless account/device state requires it.

---

# 151. Revoked Device

Routes to:

```text
Link Again
Recover Account
```

not generic first-run create account unless user chooses.

---

# 152. Partially Completed Setup

Resume semantic state.

---

# 153. Migration from Older App Version

Do not force onboarding again for mere schema migration.

---

# 154. Feature Education After Onboarding

Use contextual education.

Examples:

```text
first voice note
first nearby search
first SOS
first backup
```

---

# 155. Do Not Frontload All Feature Tutorials

Hard rule.

---

# 156. Coach Marks

Use sparingly.

Never obscure core UI or trap assistive tech.

---

# 157. Feature Education Pattern

```text
user invokes feature
→ short explanation
→ permission if needed
→ feature starts
```

---

# 158. Calls First Use

```text
Start call
→ microphone/camera education
→ OS permission
→ call
```

---

# 159. Nearby First Use

```text
Find Nearby
→ nearby education
→ OS permission
→ scan
```

---

# 160. SOS First Use

Prefer setup/test flow before real emergency use, but real SOS must not become blocked by lengthy tutorial.

---

# 161. Emergency Permission Readiness

Settings can preconfigure:

```text
notifications
location
nearby
```

without forcing all during general onboarding.

---

# 162. Backup First Use

Explain:

```text
where backup is stored
how it is protected
what recovery key means
```

at first backup, not app first launch.

---

# 163. Security Center First Use

Can show concise explanation of devices/recovery.

---

# 164. Plugin First Use

Plugin permission education belongs to Part 19.

---

# 165. Permission Copy Consistency

The same capability should use the same core explanation across onboarding/settings/feature prompt.

---

# 166. Capability Registry

Recommended central semantic registry.

```rust
pub struct CapabilityDescriptor {
    pub capability: AppCapability,
    pub purpose: CapabilityPurpose,
    pub optional: bool,
    pub fallback: Option<CapabilityAlternative>,
}
```

---

# 167. One Source of Truth for Permission Meaning

Avoid separate wording/semantics implemented independently by:

```text
Compose screen
Dioxus dialog
Settings screen
Feature screen
```

---

# 168. Platform Copy Adaptation

Natural wording can differ while meaning stays same.

---

# 169. Readiness Diagnostics

Part 20 can inspect capability readiness.

---

# 170. Settings Integration

Part 18 exposes all permission/capability statuses after onboarding.

---

# 171. Security Integration

Part 15 handles recovery/device security.

---

# 172. Device Linking Integration

Part 12 handles trust/bootstrap.

---

# 173. Backup Integration

Part 16 handles data restore.

---

# 174. Accessibility Integration

Part 21 is mandatory.

---

# 175. Design System Integration

Part 22 provides onboarding components/tokens.

---

# 176. Responsive Integration

Part 23 adapts wizard/list/detail presentation.

---

# 177. Error-State Integration

Part 24 handles permission/network/setup failures.

---

# 178. Desktop Layout

Wide:

```text
centered onboarding panel
optional explanatory side panel
```

---

# 179. Desktop Compact

Single focused panel.

---

# 180. Android Phone Layout

Full-screen step flow.

---

# 181. Android Tablet/Foldable

Can use:

```text
step list/illustration
+
current form
```

if it improves clarity.

---

# 182. No Decorative Dependence

Illustrations are optional.

Core meaning in text/actions.

---

# 183. Progress Indicator Accessibility

Screen reader:

```text
Step 2 of 4, Profile
```

if using step count.

---

# 184. Skip Link

Optional step has visible:

```text
Skip for now
```

not hidden tiny text.

---

# 185. Back Link

Preserves completed secure state.

---

# 186. Account Creation Success

Do not immediately hide recovery recommendation.

---

# 187. Account Creation Failure

If identity was not committed:

```text
retry safely
```

If committed:

```text
resume setup
```

---

# 188. Link Session Expiry

Show:

```text
Link code expired
Create/scan a new code
```

---

# 189. Recovery Wrong Key

```text
Could not unlock recovery
```

not cryptographic details.

---

# 190. Unsupported Backup Version

Route:

```text
Update App
```

---

# 191. Permission Unavailable

Example:

```text
This device has no NFC
```

Hide/disable feature gracefully.

---

# 192. Capability Readiness Screen

Optional post-onboarding:

```text
Notifications: Ready
Nearby: Not enabled
Microphone: Ask when needed
Recovery: Not configured
```

---

# 193. Recommended Readiness Policy

Do not label optional missing permissions as:

```text
Setup incomplete
```

unless a user-selected feature depends on them.

---

# 194. Completion Criteria

Account/device ready is enough for onboarding completion.

---

# 195. Welcome Back

Returning after recovery/link can show:

```text
Device ready
```

then enter home.

---

# 196. Testing Matrix

Required:

```text
new account online
new account offline
link existing device
manual-code link
recovery
skip recovery
notification denied
camera denied
microphone denied
nearby denied
location denied
```

---

# 197. Android Tests

Verify:

```text
runtime permission prompt timing
Don't Ask Again/system blocked
process death
rotation
TalkBack
large text
RTL
```

---

# 198. Desktop Tests

Verify:

```text
keyboard-only
manual code
optional webcam
system notification/camera/mic settings
restart/resume
```

---

# 199. Offline Tests

Verify:

```text
local account creation
local profile setup
nearby/device link if supported
deferred network registration
```

---

# 200. Security Tests

Verify:

```text
duplicate create command
duplicate link approval
expired link
wrong SAS
wrong recovery key
revoked device
```

---

# 201. Permission Tests

No permission requested before contextual education.

---

# 202. Accessibility Tests

Complete first run:

```text
without camera
with TalkBack
keyboard-only
200% text
RTL
```

---

# 203. Analytics Privacy Tests

No recovery key/invite code/profile secret in telemetry.

---

# 204. Resume Tests

Crash/restart at each semantic onboarding state.

---

# 205. Capability Consistency Tests

Same permission meaning/fallback appears in onboarding/settings/feature prompt.

---

# 206. Initial Production Scope

Ship:

```text
Create New Account
Use Existing Account
Link Existing Device
Recover Existing Account
minimal profile setup
recovery recommendation
contextual permission education
notification/camera/microphone/nearby/location handling
manual-code alternatives
offline-first setup
resume after process death
post-onboarding readiness card
```

Defer:

```text
long guided tours
social contact import by default
mandatory profile completion
bulk permission prompts
gamified setup completion
```

---

# 207. Definition of Done

UI/UX Part 25 is complete when:

- first-run clearly distinguishes create, link, and recover
- linking delegates to Part 12 and recovery delegates to Parts 15/16
- account/device security truth remains Rust-owned
- only truly required state blocks entry to the main app
- optional profile/recovery/permissions can be deferred where safe
- permissions are requested just in time after contextual education
- every optional permission has an explicit fallback where possible
- Android notification/camera/mic/nearby/location flows are defined
- desktop camera/mic/notification equivalents are normalized without fake authority prompts
- offline-first account creation/setup works where backend permits
- no-Internet state does not block local setup unnecessarily
- process death/restart resumes semantic onboarding state
- permission denial never causes nag loops
- post-onboarding readiness surfaces skipped configuration without blocking product use
- accessibility, keyboard, TalkBack, large text, RTL, reduced motion, and QR/manual-code alternatives are explicit
- Rust onboarding/capability/readiness APIs and events are specified
- duplicate command, recovery, permission, offline, resume, and accessibility test matrices are included

---

# 208. Final Architecture

```text
                        FIRST RUN
                            │
          ┌─────────────────┼─────────────────┐
          │                 │                 │
       Create              Link            Recover
          │                 │                 │
     New Identity      Trusted Device     Recovery Proof
          │                 │                 │
          └─────────────────┼─────────────────┘
                            │
                      Device Ready
                            │
                       Minimal Profile
                            │
                 Optional Capability Setup
                            │
                            ▼
                           Home
```

Permission path:

```text
User chooses feature
        │
        ▼
Explain why permission is needed
        │
        ▼
User chooses Continue
        │
        ▼
OS permission prompt
        │
   ┌────┴─────┐
   │          │
Granted     Denied
   │          │
Feature    Fallback / Settings
```

---

# 209. Final Principle

Onboarding should get users to a working, secure product quickly without front-loading every permission and configuration decision.

The correct model is:

```text
minimal secure setup
+
clear create/link/recover paths
+
contextual permission education
+
offline-first progress
+
optional setup deferred
```

not:

```text
welcome
→ ask for every permission
→ long tutorial
→ force every setting
→ finally show the product
```

This gives Dioxus desktop and Android Compose a first-run experience that is privacy-conscious, resilient, accessible, and consistent with the shared Rust security model.
