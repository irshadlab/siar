# UI/UX Part 18 — Settings, Privacy, Notifications & Data Controls UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 18  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete settings, privacy, notifications, data controls, storage, retention, auto-download, presence/receipts, calls, emergency behavior, backup/recovery links, plugin permissions, diagnostics, developer options, reset/delete flows, policy-locked controls, and Rust presentation contracts across desktop and Android.

---

# 1. Purpose

Settings should not become a dumping ground for every internal toggle.

Users need to understand:

```text
what affects this device only
what affects all their devices
what affects other people
what changes privacy
what changes storage
what changes notifications
what is controlled by the operating system
what is locked by organization policy
what can be undone
what deletes data
```

The governing principle is:

> **Settings must expose user intent, not implementation knobs.**

---

# 2. Architectural Position

```text
Rust Settings / Policy Core
        │
        ├── account settings
        ├── device settings
        ├── privacy policy
        ├── notification policy
        ├── storage policy
        ├── retention policy
        ├── plugin policy
        ├── security policy
        └── managed overrides
        │
        ▼
Settings Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Settings Scope

Every setting should declare one of:

```rust
pub enum SettingScope {
    AccountWide,
    DeviceLocal,
    ConversationScoped,
    GroupScoped,
    ManagedPolicy,
}
```

---

# 4. Why Scope Must Be Visible

A user should know whether changing:

```text
Read receipts
```

affects:

```text
this phone only
```

or:

```text
all devices
```

---

# 5. Main Settings Categories

Recommended:

```text
Account
Profile
Privacy
Notifications
Calls & Media
Files & Storage
Data & Retention
Search
Backup & Recovery
Devices & Security
Emergency
Plugins & Extensions
Appearance & Accessibility
Network & Offline
Advanced
About
```

---

# 6. Desktop Settings Layout

Recommended:

```text
Left navigation
    category list

Right pane
    settings content
```

Wide desktop:

```text
category
section
description
control
```

---

# 7. Android Settings Layout

Recommended:

```text
TopAppBar
Settings

LazyColumn
category rows
```

Each category opens its own Compose destination.

---

# 8. Tablet/Foldable

Use:

```text
category list
+
detail pane
```

---

# 9. Search Settings

Settings search is useful once categories grow.

Search terms:

```text
notifications
storage
read receipts
backup
devices
privacy
```

---

# 10. Settings Search Is Metadata Search

Do not reuse content search index for raw setting values.

---

# 11. Settings Row Anatomy

Recommended:

```text
Title
Short description
Current value/control
Optional scope badge
Optional policy badge
```

---

# 12. Avoid Overloaded Rows

Do not put:

```text
toggle
dropdown
button
status
```

all on one line if it harms clarity.

---

# 13. Account Settings

Potential:

```text
Account identity
Display name
Avatar
Linked devices
Recovery
Sign out/remove device
```

---

# 14. Profile Settings

Potential:

```text
Display name
Avatar
Status
About text
Visibility
```

---

# 15. Privacy Settings

Recommended sections:

```text
Presence
Typing
Read Receipts
Profile Visibility
Notification Previews
Blocked Contacts
Link/Media Privacy
Search Privacy
App Lock
```

---

# 16. Presence Controls

From Part 14:

```text
Show online status
Show last seen
Share typing
Share read receipts
```

---

# 17. Account-Wide Privacy

Recommended default:

```text
presence/read/typing policy account-wide
```

unless product explicitly chooses device-local.

---

# 18. Per-Contact Overrides

Future.

Do not complicate v1 unless strong need.

---

# 19. Profile Visibility

Potential:

```text
Contacts
People I message
Nobody
```

depending product model.

---

# 20. Notification Preview Privacy

Recommended:

```text
Full preview
Sender only
Generic
```

plus:

```text
Hide previews when device locked
```

---

# 21. App Lock

Potential:

```text
Off
Immediately
After 1 minute
After 5 minutes
After 30 minutes
```

---

# 22. Android App Lock

Can use:

```text
BiometricPrompt
device credential
```

for unlock.

---

# 23. Desktop App Lock

Can use:

```text
local passphrase
OS authentication
```

depending architecture.

---

# 24. Blocked Contacts

Dedicated list:

```text
Blocked
Unblock
```

---

# 25. Notification Settings

From Part 13.

Recommended:

```text
Messages
Mentions
Message Requests
Calls
Security
Transfers
Backup
Emergency
Quiet Hours
Preview Privacy
```

---

# 26. Android Notification System Ownership

For channel-level controls:

```text
sound
vibration
importance
```

link to Android system settings.

---

# 27. App-Level Notification Policy

App can still own:

```text
mute semantics
mentions
quiet hours
conversation overrides
```

---

# 28. Notification Permission Status

Show:

```text
Allowed
Denied
Blocked by system
```

---

# 29. Call Notification Warning

If disabled:

```text
Incoming calls may not alert you in the background.
```

---

# 30. Quiet Hours

Potential:

```text
Start
End
Days
Exceptions
```

---

# 31. Quiet Hour Exceptions

Potential:

```text
Calls
Security
Emergency
Mentions
Favorite contacts
```

---

# 32. Conversation Notification Overrides

Do not list thousands inline.

Use:

```text
Muted Conversations
```

summary and management screen.

---

# 33. Calls & Media Settings

Recommended:

```text
Microphone
Speaker
Camera
Call ringtone
Video quality
Data-saving mode
Hardware acceleration
Screen sharing privacy
Call background behavior
```

---

# 34. Desktop Audio Device Settings

Separate:

```text
Microphone
Speaker
Ringer
```

---

# 35. Android Audio Route

Mostly runtime/system controlled.

Settings may include preference:

```text
Default to speaker for video calls
```

only if product needs it.

---

# 36. Camera Preference

Desktop:

```text
default camera
```

Android:

```text
front camera preferred
```

---

# 37. Video Quality

Recommended user-level choices:

```text
Auto
Data Saver
High Quality
```

Not:

```text
bitrate 1840 kbps
```

in normal settings.

---

# 38. Hardware Acceleration

Advanced or diagnostic setting.

---

# 39. Calls on Metered Network

Potential:

```text
Allow video on mobile data
```

Android.

---

# 40. Files & Storage

Recommended:

```text
Storage Usage
Auto-Download
Save Location / Export Defaults
Media Quality
Clear Cache
Partial Transfers
```

---

# 41. Storage Usage

Show:

```text
Messages metadata
Media
Documents
Audio
Thumbnails
Search index
Partial transfers
```

---

# 42. Storage by Conversation

Link to Part 10.

---

# 43. Auto-Download

Recommended:

```text
Never
Wi-Fi only
Small files
Always
```

---

# 44. Per-Type Auto-Download

Optional:

```text
Images
Video
Audio
Documents
```

---

# 45. Android Metered Awareness

Use platform network status.

---

# 46. Desktop Auto-Download

Can default more permissively.

---

# 47. Clear Cache

Safe derived content:

```text
thumbnails
temporary previews
search semantic cache
```

---

# 48. Clear Local Media

Different from cache.

Must explain:

```text
Files can be downloaded again if still available.
```

---

# 49. Partial Transfers

Allow:

```text
Clear partial downloads
```

---

# 50. Data & Retention

Recommended:

```text
Keep Messages
Keep Downloaded Media
Delete Local Copies After
Draft Retention
Emergency History Retention
Call History Retention
```

---

# 51. Message Retention

Potential:

```text
Forever
1 year
6 months
90 days
```

only if backend supports local deletion semantics safely.

---

# 52. Local vs Global Deletion

Hard distinction.

A retention policy may mean:

```text
remove local copies
```

not:

```text
delete from everyone
```

---

# 53. Retention Description

Example:

```text
Messages older than 1 year will be removed from this device when safe.
```

---

# 54. Group/Conversation Retention

Future per-conversation override.

---

# 55. Emergency History Retention

Potentially more privacy-sensitive.

---

# 56. Call History Retention

Separate from call content/media.

---

# 57. Draft Retention

Durable drafts may be kept until sent/deleted.

---

# 58. Search Settings

Recommended:

```text
Index Messages
Index Filenames
Index File Contents
Semantic Search
Search History
Rebuild Index
Clear Index
```

---

# 59. Default Search Policy

Enable local lexical indexing.

---

# 60. Semantic Search

Optional.

Explain:

```text
Uses additional local storage and processing.
```

---

# 61. External AI Search

If configured:

```text
explicit provider
explicit data scope
```

---

# 62. Search History

Device-local recommended.

---

# 63. Rebuild Index

Advanced action.

---

# 64. Backup & Recovery

Links to Parts 15/16:

```text
Backup Status
Back Up Now
Automatic Backup
Recovery Method
Verify Backup
Restore
Export
```

---

# 65. Avoid Duplicating Security Logic

Settings routes to Security/Backup screens.

Do not reimplement recovery key operations.

---

# 66. Devices & Security

Recommended:

```text
Trusted Devices
Security Center
App Lock
Verification
Recovery
Security Events
```

---

# 67. Emergency Settings

From Part 17:

```text
Emergency Contacts
SOS Preset
Location Sharing
Cancel Countdown
Emergency Alerts
Offline Relay Participation
Battery/Data Policy
Test SOS
```

---

# 68. Emergency Contact Warning

Settings should clearly mark:

```text
unverified emergency contact
```

---

# 69. Relay Participation

Potential:

```text
Allow emergency relay for others
```

with:

```text
battery minimum
charging-only
data usage
```

---

# 70. Plugins & Extensions

Recommended:

```text
Installed Plugins
Permissions
Enabled/Disabled
Update Status
Storage/Network Access
Remove Plugin
```

---

# 71. Plugin Permission Categories

Potential:

```text
Messages
Contacts
Files
Search
Network
Notifications
Commands
UI Extensions
```

---

# 72. Plugin Permission Principle

Use capability-level permissions.

Avoid broad:

```text
Full Access
```

unless truly necessary.

---

# 73. Plugin Permission Change

High-impact permission additions require confirmation.

---

# 74. Plugin Disabled

Plugin data may remain.

Explain retention/removal separately.

---

# 75. Remove Plugin

Separate:

```text
Remove plugin
Delete plugin data
```

where applicable.

---

# 76. Appearance

Recommended:

```text
System
Light
Dark
```

---

# 77. Accent Color

Optional.

---

# 78. Desktop Density

Potential:

```text
Comfortable
Compact
```

---

# 79. Android Dynamic Color

Optional Material 3 setting.

---

# 80. Typography

Potential:

```text
System
Small
Default
Large
```

but should still respect OS accessibility font scaling.

---

# 81. Reduced Motion

Respect OS setting by default.

Optional explicit app override:

```text
Follow system
Reduce motion
```

---

# 82. Accessibility Settings

Potential:

```text
High contrast
Reduce motion
Larger controls
Always show labels
Message delivery text
Typing animation off
```

Only add when system defaults insufficient.

---

# 83. Network & Offline

Recommended:

```text
Relay policy
LAN discovery
Nearby discovery
Offline mesh
Data Saver
Proxy/Tor if supported
Preferred network behavior
```

---

# 84. Normal User Language

Prefer:

```text
Use nearby devices when Internet is unavailable
```

instead of:

```text
Enable DTN
```

---

# 85. Advanced Network

Developer/advanced users can see:

```text
Direct connections
Relay
LAN
Mesh
```

---

# 86. Data Saver

Can reduce:

```text
video quality
auto-download
presence update rate
background transfer
```

through policy engine.

---

# 87. Offline Mode

Potential manual mode:

```text
Local only
```

if architecture supports.

---

# 88. Local-Only Mode

Explain:

```text
Use only nearby/LAN connections and do not use Internet relays.
```

---

# 89. Proxy

Only if implemented.

---

# 90. Advanced Settings

Potential:

```text
Diagnostics
Logs
Protocol details
Experimental features
Developer mode
Reset derived data
```

---

# 91. Experimental Features

Must be clearly separated from stable settings.

---

# 92. Feature Flags

Rust owns actual availability.

---

# 93. Developer Mode

Can unlock:

```text
network path diagnostics
raw IDs
performance counters
plugin debug
```

---

# 94. Developer Mode Warning

Example:

```text
Advanced diagnostics may expose technical metadata.
```

---

# 95. Logs

Normal users:

```text
Export diagnostics
```

not browse raw logs unless useful.

---

# 96. Diagnostic Export

Must redact:

```text
message content
keys
recovery material
full contact IDs where possible
```

---

# 97. Reset Derived Data

Safe actions:

```text
Rebuild search index
Clear thumbnails
Clear cache
Reset layout
```

---

# 98. Reset Settings

Potential:

```text
Reset Device Settings
Reset All Account Preferences
```

Keep scopes distinct.

---

# 99. Reset Device Settings

Could reset:

```text
layout
notifications local preferences
download policy
appearance
```

---

# 100. Reset Account Preferences

Account-wide privacy/settings.

High-impact confirmation.

---

# 101. Sign Out / Remove This Device

Routes to Part 15.

---

# 102. Delete Account

Only if product has account deletion semantics.

Must be isolated under:

```text
Danger Zone
```

---

# 103. Account Deletion Is Not Local Data Deletion

Hard distinction.

---

# 104. Delete Local Data

Separate:

```text
Remove local data from this device
```

---

# 105. Clear Conversation Data

Separate per conversation.

---

# 106. Export Before Delete

Offer:

```text
Back Up / Export
```

before destructive account/local deletion.

---

# 107. Delete Account Confirmation

Must explain:

```text
what is deleted
what may remain on other users' devices
what is irreversible
```

---

# 108. Remote Copies Honesty

Do not promise deletion of content already:

```text
exported
backed up by peer
retained by another user
```

unless protocol guarantees.

---

# 109. Managed Policy

Enterprise/school/admin-managed deployments may lock settings.

---

# 110. Managed Setting State

```rust
pub enum SettingControlState<T> {
    Editable(T),
    Managed(T),
    Unavailable,
}
```

---

# 111. Managed UI

Show:

```text
Managed by organization
```

with explanation.

---

# 112. Do Not Hide Managed Setting

Better to show current value and why it cannot change.

---

# 113. Policy Conflict

If account preference differs from enforced policy:

```text
effective value = policy
```

Rust reports effective state.

---

# 114. Effective Setting

```rust
pub struct EffectiveSetting<T> {
    pub requested: Option<T>,
    pub effective: T,
    pub source: SettingValueSource,
}
```

---

# 115. Value Source

```rust
pub enum SettingValueSource {
    User,
    DeviceDefault,
    AccountDefault,
    ManagedPolicy,
    System,
}
```

---

# 116. Settings Persistence

Rust owns durable configuration.

---

# 117. UI Does Not Persist Settings Independently

No Kotlin `SharedPreferences` for domain policy unless explicitly device-platform-only.

---

# 118. Android Platform Preferences

Some settings may be platform-only:

```text
dynamic color
notification channel sound
PiP preference
```

These can live in Android adapter layer.

---

# 119. Desktop Platform Preferences

Examples:

```text
start minimized
tray behavior
window layout
```

---

# 120. Shared vs Platform Settings

Use clear namespaces.

---

# 121. Settings Snapshot

```rust
pub struct SettingsSnapshot {
    pub account: AccountSettingsView,
    pub privacy: PrivacySettingsView,
    pub notifications: NotificationSettingsView,
    pub media: MediaSettingsView,
    pub storage: StorageSettingsView,
    pub retention: RetentionSettingsView,
    pub search: SearchSettingsView,
    pub emergency: EmergencySettingsView,
    pub network: NetworkSettingsView,
}
```

---

# 122. Setting Descriptor

```rust
pub struct SettingDescriptor<T> {
    pub key: SettingKey,
    pub value: T,
    pub scope: SettingScope,
    pub control: SettingControlState<T>,
    pub restart_required: bool,
}
```

---

# 123. Typed Setting Keys

Avoid stringly typed:

```text
"privacy.read_receipts"
```

inside UI logic.

Use Rust enums/types.

---

# 124. Settings Presentation API

```rust
pub trait SettingsPresentation {
    async fn snapshot(
        &self,
    ) -> Result<SettingsSnapshot, UiError>;

    async fn update(
        &self,
        command: SettingsUpdateCommand,
    ) -> Result<SettingsUpdateResult, UiError>;

    async fn reset(
        &self,
        scope: SettingsResetScope,
    ) -> Result<SettingsSnapshot, UiError>;
}
```

---

# 125. Settings Update Command

```rust
pub enum SettingsUpdateCommand {
    Privacy(PrivacySettingsUpdate),
    Notifications(NotificationSettingsUpdate),
    Media(MediaSettingsUpdate),
    Storage(StorageSettingsUpdate),
    Retention(RetentionSettingsUpdate),
    Search(SearchSettingsUpdate),
    Emergency(EmergencySettingsUpdate),
    Network(NetworkSettingsUpdate),
}
```

---

# 126. Update Result

```rust
pub struct SettingsUpdateResult {
    pub applied: bool,
    pub effective_value_changed: bool,
    pub restart_required: bool,
    pub warning: Option<SettingsWarningView>,
}
```

---

# 127. Optimistic Settings

Low-risk settings can update optimistically:

```text
theme
view density
```

---

# 128. High-Risk Settings

Wait for Rust confirmation:

```text
privacy
retention
emergency relay
security-sensitive plugin permissions
```

---

# 129. Setting Side Effects

Some setting changes trigger:

```text
stop presence publishing
clear search history
restart discovery
cancel auto-download
```

Rust coordinates side effects.

---

# 130. UI Must Not Implement Side Effects Itself

Example:

```text
turn off read receipts
```

should not require Compose/Dioxus to manually cancel network tasks.

---

# 131. Restart Required

Rare.

If needed:

```text
Restart required
Restart now
Later
```

---

# 132. Prefer Live Reconfiguration

Most settings should apply without restart.

---

# 133. Privacy Change Propagation

Account-wide changes sync to trusted devices.

---

# 134. Device-Local Change

Do not sync:

```text
window layout
Android dynamic color
local notification sound
download-on-mobile-data preference if intended local
```

---

# 135. Settings Conflict Across Devices

Account-wide settings use versioned authoritative update.

---

# 136. Stale Settings Edit

If another device updates same setting while screen open:

```text
Rust resolves
UI receives new effective value
```

---

# 137. Settings Event Model

```rust
pub enum SettingsUiEvent {
    SettingChanged(SettingKey),
    PolicyChanged(SettingKey),
    PlatformCapabilityChanged(PlatformCapability),
}
```

---

# 138. Platform Capability

Examples:

```text
Notification permission revoked
Camera unavailable
NFC disabled
Storage provider disconnected
```

---

# 139. Android ViewModel

Owns:

```text
navigation
dialogs
system-settings effects
temporary text fields
```

Rust owns effective configuration.

---

# 140. Dioxus Presenter

Owns:

```text
category selection
search
dialogs
window-level platform preferences
```

---

# 141. Platform Settings Effects — Android

Examples:

```text
OpenNotificationSettings
OpenBatterySettings
OpenAppPermissions
RequestBiometric
OpenStoragePicker
```

---

# 142. Platform Settings Effects — Desktop

Examples:

```text
OpenSystemNotificationSettings
OpenLogFolder
ChooseDownloadFolder
RestartApp
```

---

# 143. No Fake OS Controls

If Android owns notification sound:

```text
Open Android Settings
```

rather than showing a fake in-app sound toggle that cannot enforce it.

---

# 144. Data Control UX

Users should be able to answer:

```text
What data is on this device?
What can I delete safely?
What is synced?
What is backed up?
What is exported?
```

---

# 145. Data Control Overview

Recommended:

```text
Local Storage
Synced Data
Backups
Exports
Search Index
Cache
```

---

# 146. Local Storage

Shows device-specific usage.

---

# 147. Synced Data

Conceptual account data.

Do not confuse with physical byte duplication.

---

# 148. Backups

Link to Part 16.

---

# 149. Exports

External copies are outside app control.

---

# 150. Search Index

Derived/rebuildable.

---

# 151. Cache

Derived/disposable.

---

# 152. Delete Local Copy

Safe if remotely available.

---

# 153. Delete Everywhere

Only where backend supports and user has authority.

---

# 154. Data Controls Must State Scope

Every destructive action labels:

```text
This device
All your devices
Conversation
Everyone
```

---

# 155. Storage Cleanup Wizard

Potential:

```text
Large Files
Old Media
Partial Transfers
Cache
Search Index
```

with estimated space.

---

# 156. Cleanup Confirmation

Show:

```text
4.2 GB will be freed
Messages remain
```

---

# 157. Retention Preview

Before changing:

```text
Approximately 12 GB may be removed over time
```

if computable.

---

# 158. Data Export

Links to Part 16.

---

# 159. Data Import

Links to Part 16.

---

# 160. Download Your Data

If account/server product supports server-side export, separate from local export.

Not implied by current architecture.

---

# 161. Privacy Summary

Potential card:

```text
Presence: Contacts
Read receipts: Off
Notification previews: Generic
Search: Local only
```

---

# 162. Privacy Quick Audit

Future:

```text
Review privacy
```

step-through.

---

# 163. Do Not Use Shame Language

Avoid:

```text
Your privacy is weak
```

Prefer factual state.

---

# 164. Settings Help Text

Keep concise.

Provide deeper explanation via:

```text
Learn more
```

or details.

---

# 165. Accessibility — Settings

Every toggle has:

```text
name
description
state
scope if important
```

---

# 166. Screen Reader

Example:

```text
Read receipts, off, applies to all devices
```

---

# 167. Managed Setting Accessibility

```text
Read receipts, off, managed by organization
```

---

# 168. Large Font

Rows expand vertically.

Do not truncate explanatory text.

---

# 169. RTL

Settings layouts mirror.

Technical identifiers remain canonical.

---

# 170. Reduced Motion

No dependency on animated toggles.

---

# 171. Color Independence

Warnings/managed states use text/icons.

---

# 172. Keyboard Desktop

Support:

```text
Ctrl/Cmd+F → Search settings
Tab
Arrow navigation
Space → toggle
Enter → open
```

---

# 173. Android TalkBack

Settings rows use native Compose semantics.

---

# 174. Settings Search Accessibility

Search result announces category/path.

Example:

```text
Read receipts, Privacy
```

---

# 175. Error Handling

If update fails:

```text
restore previous effective value
show inline/snackbar error
```

---

# 176. Policy Rejection

Example:

```text
This setting is managed by your organization.
```

---

# 177. Offline Settings Update

Device-local settings work immediately.

Account-wide settings can:

```text
apply locally
queue sync
```

if backend semantics allow.

---

# 178. Offline Policy Conflict

If server/authority later rejects:

```text
Rust restores effective value
UI explains
```

---

# 179. Security-Sensitive Offline Change

Could require online confirmation.

Example:

```text
delete account
change recovery policy
```

---

# 180. Settings Versioning

Rust migrations handle old config versions.

UI should not care.

---

# 181. Unknown Old Setting

Safe defaults.

---

# 182. Reset After Upgrade

Avoid silently resetting user privacy choices.

---

# 183. Backup of Settings

Account/device settings included according to Part 16 scope.

---

# 184. Restore of Settings

Device-specific settings should not blindly overwrite platform-specific preferences on different device type.

---

# 185. Cross-Platform Restore

Example:

```text
desktop window density
```

should not map to Android.

---

# 186. Shared Setting Migration

Only shared semantic settings restore cross-platform.

---

# 187. Notification Channel Restore

Android channel settings are system-owned.

Do not fake restore.

---

# 188. Theme Restore

Could be account-wide or device-local.

Recommendation:

```text
device-local
```

unless users explicitly want sync.

---

# 189. Accessibility Setting Restore

Prefer device-local/system-linked.

---

# 190. Emergency Settings Restore

Emergency contacts may be account-wide.

Location permission is device-local and must be requested again.

---

# 191. Plugin Settings Restore

Only if plugin installed and compatible.

---

# 192. Missing Plugin

Preserve dormant config safely or discard per policy.

---

# 193. Settings Telemetry

Do not log sensitive values:

```text
blocked contacts
emergency contacts
privacy choices linked to identity
```

without explicit product need.

---

# 194. Safe Metrics

Possible:

```text
settings screen latency
update failure rate
policy conflict rate
```

without values/content.

---

# 195. Crash Reports

Redact:

```text
contact IDs
paths
recovery data
plugin secrets
```

---

# 196. Diagnostics Settings

Allow:

```text
Export Diagnostics
View Network Status
Rebuild Search
Reset Layout
```

---

# 197. Developer Settings

Potential:

```text
Protocol logging level
Show IDs
Experimental transports
UI performance overlay
```

---

# 198. Developer Logging

Never allow enabling plaintext secret/message logs accidentally.

---

# 199. Unsafe Developer Toggle

If a diagnostic can expose sensitive data:

```text
strong warning
temporary enable
```

---

# 200. About

Recommended:

```text
Version
Build
Licenses
Privacy Policy
Terms if applicable
Open Source Licenses
Diagnostic Version Info
```

---

# 201. License UX

Open-source license notices can be searchable/listed.

---

# 202. Update Status

Desktop may show:

```text
Check for updates
```

if updater exists.

Android typically Play/system distribution.

---

# 203. Update Channel

Advanced:

```text
Stable
Beta
```

only if supported.

---

# 204. Settings Testing Matrix

Required:

```text
account-wide setting
device-local setting
managed setting
offline update
policy conflict
search
reset
notification permission
storage cleanup
privacy toggle
retention
plugin permission
```

---

# 205. Android Tests

Verify:

```text
system notification settings
app permissions
SAF
BiometricPrompt
process death
TalkBack
large font
RTL
```

---

# 206. Desktop Tests

Verify:

```text
two-pane settings
keyboard
native folder picker
system notification settings
window settings
restart-required path
```

---

# 207. Multi-Device Tests

Change account-wide privacy on phone.

Desktop updates.

Change desktop density.

Phone does not change.

---

# 208. Managed Policy Tests

Policy locks setting.

UI shows effective value and reason.

---

# 209. Reset Tests

Device reset does not erase account-wide privacy.

Account preference reset does not delete messages.

---

# 210. Data Cleanup Tests

Clear cache:

```text
messages remain
```

Delete local media:

```text
metadata remains
```

Delete everywhere:

```text
only where authorized
```

---

# 211. Accessibility Tests

Complete all categories with screen reader/keyboard.

---

# 212. Performance

Settings snapshot should load quickly from local state.

---

# 213. Lazy Sections

Expensive diagnostics/storage calculations can load asynchronously.

---

# 214. Do Not Block Settings UI

Example:

```text
calculating 400 GB storage usage
```

should not freeze navigation.

---

# 215. Stable Keys

Use:

```text
SettingKey
```

for row identity.

---

# 216. Initial Production Scope

Ship:

```text
Account/Profile
Privacy
Notifications
Calls & Media
Files & Storage
Data & Retention
Search
Backup & Recovery links
Devices & Security links
Emergency
Appearance
Network & Offline
Advanced diagnostics
About
```

Defer:

```text
hundreds of per-contact overrides
deep transport tuning
custom policy DSL
complex theme editor
experimental toggles exposed to all users
```

---

# 217. Definition of Done

UI/UX Part 18 is complete when:

- every setting has an explicit scope: account-wide, device-local, conversation/group, or managed
- settings categories are coherent and not implementation-driven
- privacy, notification, media, storage, retention, search, emergency, plugin, appearance, network, and advanced settings are defined
- Android system-owned settings route to platform settings rather than fake in-app controls
- device-local and account-wide settings sync semantics are explicit
- managed policy shows effective value and lock reason
- destructive data controls clearly distinguish this device, all devices, conversation, and everyone
- cache/search indexes/thumbnails are distinguished from authoritative data
- retention semantics never silently imply delete-for-everyone
- backup/recovery/security settings route to their authoritative Part 15/16 surfaces
- plugin permissions are capability-based
- settings updates and side effects are Rust-authoritative
- high-risk settings wait for Rust confirmation
- cross-platform restore does not blindly restore platform-specific preferences
- accessibility, RTL, large font, keyboard/TalkBack, and managed-state semantics are explicit
- typed Rust settings models, update commands, effective-value sources, and events are specified
- offline, policy-conflict, reset, multi-device, cleanup, and platform-permission tests are included

---

# 218. Final Architecture

```text
                  RUST SETTINGS / POLICY CORE
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
     Account              Device             Managed Policy
        │                    │                    │
     Privacy             Storage             Effective Value
     Receipts            Appearance          Locked Controls
     Emergency           Platform Prefs
        │                    │                    │
        └────────────────────┼────────────────────┘
                             │
                   Settings Presentation
                    ┌────────┴────────┐
                    │                 │
                 Dioxus            Compose
                    │                 │
            Desktop Settings    Android Settings
```

Every setting resolves to:

```text
requested value
+
scope
+
policy/system constraints
=
effective value
```

---

# 219. Final Principle

A good settings system lets users control the product without requiring them to understand its internals.

The correct model is:

```text
user intent
+
clear scope
+
clear consequences
+
effective policy
+
Rust-owned semantics
```

not:

```text
hundreds of loosely related toggles backed by separate UI-local preferences
```

This keeps Dioxus desktop and Android Compose consistent while still allowing each platform to own the settings that genuinely belong to the operating system.
