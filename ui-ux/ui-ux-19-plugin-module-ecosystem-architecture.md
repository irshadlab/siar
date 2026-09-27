# UI/UX Part 19 — Plugin / Module Ecosystem UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 19  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete plugin/module ecosystem UX across desktop and Android, including discovery, install, trust, signing, capability permissions, first-run consent, enable/disable/remove, updates, compatibility, sandbox/resource limits, plugin settings, UI extension points, commands, search, composer integration, background work, failures, data retention, enterprise policy, developer workflows, accessibility, and the Rust presentation contracts that keep extensions from bypassing core security.

---

# 1. Purpose

A plugin ecosystem can make the platform significantly more useful, but it also creates a new trust boundary.

Plugins may want to:

```text
read messages
index files
add commands
extend search
add composer actions
show UI panels
send notifications
use the network
store data
run background tasks
interact with contacts
```

The UX must make these capabilities understandable before they are granted.

The governing principle is:

> **Plugins extend product capability, not product authority. Core security, identity, messaging, storage, and trust rules remain owned by Rust.**

---

# 2. Architectural Position

```text
Plugin Package
     │
     ▼
Rust Plugin Runtime
     │
     ├── manifest validation
     ├── signature / provenance
     ├── compatibility checks
     ├── capability sandbox
     ├── resource quotas
     ├── lifecycle
     ├── storage namespace
     └── extension registry
     │
     ▼
Plugin Presentation Service
     │
   ┌─┴─────────────┐
   │               │
Dioxus          Compose
Desktop         Android
```

---

# 3. Plugin UX Surfaces

Recommended:

```text
Plugins
Installed
Discover
Updates
Permissions
Developer
```

Optional:

```text
Recommended
Organization Approved
Local Packages
```

---

# 4. Plugin Categories

Potential:

```rust
pub enum PluginCategory {
    Productivity,
    Search,
    FileTools,
    Communication,
    Automation,
    Integrations,
    Accessibility,
    DeveloperTools,
    Other,
}
```

---

# 5. Plugin Identity

Every plugin has a stable:

```text
PluginId
```

Do not use:

```text
display name
package filename
publisher string
```

as identity.

---

# 6. Plugin Summary

```rust
pub struct PluginSummaryView {
    pub id: PluginId,
    pub name: String,
    pub version: PluginVersion,
    pub publisher: PluginPublisherView,
    pub state: PluginInstallState,
    pub trust: PluginTrustView,
    pub capabilities: Vec<PluginCapabilitySummary>,
    pub update: Option<PluginUpdateView>,
}
```

---

# 7. Installation States

```rust
pub enum PluginInstallState {
    NotInstalled,
    Installing,
    InstalledDisabled,
    InstalledEnabled,
    UpdateAvailable,
    Incompatible,
    Quarantined,
    Failed,
}
```

---

# 8. Plugin Trust

```rust
pub enum PluginTrustState {
    VerifiedPublisher,
    SignedUnknownPublisher,
    Unsigned,
    LocalDeveloper,
    Revoked,
}
```

---

# 9. Verified Publisher

Means:

```text
package signature/provenance verified to a known publisher
```

It does not mean:

```text
plugin is harmless
```

---

# 10. Unsigned Plugin

High-risk.

Normal users should not install by default.

---

# 11. Local Developer Plugin

Only available in Developer Mode.

---

# 12. Revoked Plugin

Should not run.

---

# 13. Discover Screen

Recommended card content:

```text
name
short description
publisher
category
permissions summary
version
compatibility
install
```

---

# 14. Do Not Hide Permissions Until After Install

Important capability summary must be visible before install.

---

# 15. Plugin Detail Screen

Recommended sections:

```text
Overview
Permissions
Data Access
UI Extensions
Background Activity
Version & Compatibility
Publisher
Privacy
Changelog
Install / Enable / Remove
```

---

# 16. Permission Model

Use capabilities, not vague trust.

Examples:

```rust
pub enum PluginCapability {
    ReadMessages,
    WriteMessages,
    ReadContacts,
    ReadFiles,
    WriteFiles,
    SearchProvider,
    ComposerExtension,
    CommandProvider,
    NotificationProvider,
    NetworkAccess,
    BackgroundTasks,
    UiPanel,
    GroupMetadata,
}
```

---

# 17. Capability Principle

A plugin should get:

```text
the narrowest capability it needs
```

not:

```text
full app access
```

---

# 18. Message Access

Split:

```text
ReadMessages
WriteMessages
```

Do not collapse.

---

# 19. File Access

Prefer:

```text
plugin-scoped file handles
```

rather than arbitrary filesystem access.

---

# 20. Network Access

Explicit capability.

Show domain restrictions if supported.

---

# 21. Background Tasks

Explicit.

Explain:

```text
This plugin can run while the app is in the background.
```

---

# 22. Notification Capability

Plugin can request semantic notification through core policy.

It cannot directly bypass notification privacy/security rules.

---

# 23. Search Provider

Can contribute:

```text
plugin-owned searchable items
```

and optionally consume core search only if separately permitted.

---

# 24. UI Panel

Allows declarative extension surface.

Not arbitrary access to Dioxus/Compose internals.

---

# 25. Composer Extension

Can add:

```text
action
attachment helper
structured content generator
```

but cannot directly send a message without user/core command.

---

# 26. Command Provider

Can register:

```text
command palette actions
context actions
```

with explicit metadata.

---

# 27. Permission Summary

Before install:

```text
Can read messages
Can access files you choose
Can use network
Can run in background
```

---

# 28. High-Risk Permissions

Examples:

```text
Read all messages
Write messages
Network access
Background tasks
Contacts
```

should receive stronger emphasis.

---

# 29. Scoped Permissions

Preferred where possible:

```text
This conversation only
Selected files only
Selected contacts only
All messages
```

---

# 30. Install Flow

Recommended:

```text
Plugin Detail
→ Review Publisher
→ Review Permissions
→ Install
→ First-Run Permission Confirmation
→ Enabled/Disabled
```

---

# 31. Install Does Not Automatically Mean Enabled

Recommendation:

```text
install
→ validate
→ user enables
```

for higher-risk plugins.

Low-risk signed modules may enable immediately if user explicitly installed and permissions already accepted.

---

# 32. First-Run Consent

If plugin asks for runtime capability not known at install:

```text
show permission request
```

---

# 33. No Surprise Escalation

A plugin update that requests new capability must not silently inherit it.

---

# 34. Permission Escalation Flow

```text
Update available
→ New permissions requested
→ Review
→ Accept and update
or
Keep current version
```

---

# 35. Permission Downgrade

Can apply automatically or show informational note.

---

# 36. Enable/Disable

Disable:

```text
stops plugin execution
```

but may preserve plugin data.

---

# 37. Remove

Removes runtime/package.

Plugin data handling is separate.

---

# 38. Remove Plugin Data

Offer:

```text
Keep plugin data
Delete plugin data
```

where meaningful.

---

# 39. Data Retention Warning

Explain:

```text
Removing the plugin does not automatically delete exported/external data.
```

---

# 40. Plugin Storage

Each plugin gets isolated namespace.

UI can show:

```text
Plugin data: 240 MB
```

---

# 41. Clear Plugin Data

Action:

```text
Clear Data
```

with confirmation.

---

# 42. Plugin Cache

Separate from plugin authoritative user data if architecture distinguishes.

---

# 43. Compatibility

```rust
pub enum PluginCompatibility {
    Compatible,
    RequiresAppUpdate,
    RequiresPluginUpdate,
    UnsupportedPlatform,
    IncompatibleApi,
}
```

---

# 44. Platform Compatibility

A plugin may support:

```text
Desktop only
Android only
Both
```

---

# 45. Unsupported Platform

Show:

```text
Not available on Android
```

rather than install error.

---

# 46. API Version

Normal UI:

```text
Compatible with this app version
```

Advanced:

```text
Plugin API v4
```

---

# 47. Plugin Update

States:

```text
Update available
Downloading
Verifying
Ready
Failed
```

---

# 48. Update Verification

Must verify signature/provenance before activation.

---

# 49. Atomic Update

Old version remains until new version validated.

---

# 50. Update Failure

Plugin continues on old version where safe.

---

# 51. Breaking Update

If state migration needed:

```text
plugin performs bounded migration
```

with rollback policy.

---

# 52. Update Changelog

Optional but useful.

---

# 53. Auto-Update

Settings:

```text
Auto-update verified plugins
Notify only
Manual
```

---

# 54. Unsigned Plugins

Never auto-update from arbitrary source.

---

# 55. Publisher Identity

Show:

```text
publisher name
verification state
website/source if supported
```

---

# 56. Marketplace vs Local Install

Separate sources:

```rust
pub enum PluginSourceKind {
    Marketplace,
    OrganizationCatalog,
    LocalPackage,
    DeveloperWorkspace,
}
```

---

# 57. Marketplace

Can provide:

```text
verified metadata
reviews? optional
publisher verification
update channel
```

---

# 58. Organization Catalog

Managed deployments may allow only approved plugins.

---

# 59. Local Package

Advanced flow:

```text
Install from file
```

with stronger warning.

---

# 60. Developer Workspace

Hot-reload/debug only in Developer Mode.

---

# 61. Local Install Warning

Example:

```text
This plugin was installed from a local package and has not been reviewed by a trusted catalog.
```

---

# 62. Plugin Signatures

UI should expose:

```text
Verified
Signed
Unsigned
Revoked
```

without overwhelming with cryptographic details.

---

# 63. Signature Failure

Hard block.

```text
Plugin package could not be verified
```

---

# 64. Revoked Publisher/Package

Plugin enters:

```text
Quarantined
```

and is disabled.

---

# 65. Quarantine UX

Show:

```text
This plugin was disabled for security reasons.
Review
Remove
```

---

# 66. Plugin Crash Isolation

A plugin crash must not crash core app.

---

# 67. Plugin Failure State

```rust
pub enum PluginRuntimeHealth {
    Healthy,
    Slow,
    Crashed,
    DisabledByPolicy,
    Quarantined,
}
```

---

# 68. Crashed Plugin UX

Show:

```text
Plugin stopped unexpectedly
Restart
Disable
View Diagnostics
```

---

# 69. Repeated Crash

Rust may auto-disable.

UI:

```text
Disabled after repeated crashes
```

---

# 70. Slow Plugin

If resource budget exceeded:

```text
Plugin is using too many resources
```

---

# 71. Resource Limits

Plugin runtime enforces:

```text
CPU
memory
storage
network
task concurrency
event rate
```

---

# 72. Resource Settings

Normal users should not tune numeric limits.

Use:

```text
Normal
Restricted
Allow More Resources
```

only if needed.

---

# 73. Background Resource Use

Show in plugin detail:

```text
Background activity allowed
```

---

# 74. Battery Impact Android

If plugin performs background work:

```text
May increase battery use
```

---

# 75. Data Usage

If plugin network access is high:

```text
Network access enabled
```

Optional usage stats.

---

# 76. Plugin Network Privacy

If network access exists, plugin should not automatically receive message data unless separately granted.

---

# 77. Capability Composition

Example:

```text
ReadMessages + NetworkAccess
```

is materially riskier than either alone.

UI can show:

```text
This plugin can read messages and send data over the network.
```

---

# 78. Risk Summary

Rust may compute:

```rust
pub enum PluginRiskLevel {
    Low,
    Moderate,
    High,
}
```

but avoid fear-based scoring.

---

# 79. Risk Explanation

Use facts:

```text
Can read messages
Can use Internet
Runs in background
```

---

# 80. Plugin Settings

Each plugin may expose declarative settings schema.

---

# 81. Plugin Settings Schema

Examples:

```text
toggle
choice
text
number with bounds
secret credential reference
```

---

# 82. Secret Settings

API keys/tokens stored through secure secret store.

Do not pass plaintext through ordinary plugin config file.

---

# 83. Secret Field UX

```text
Set credential
Replace credential
Remove credential
```

not show full secret.

---

# 84. Plugin Settings UI

Generated from typed schema.

Desktop Dioxus and Android Compose render native controls.

---

# 85. Plugin Cannot Inject Arbitrary Native UI

Recommendation:

```text
declarative extension model
```

for most plugins.

---

# 86. UI Extension Points

Potential:

```rust
pub enum UiExtensionPoint {
    ConversationAction,
    ComposerAction,
    MessageContextAction,
    FileAction,
    ContactAction,
    SearchProvider,
    SettingsSection,
    SidebarDestination,
    InspectorPanel,
}
```

---

# 87. Sidebar Destination

High-impact.

Only selected/approved plugins should add top-level navigation.

---

# 88. Conversation Action

Example:

```text
Translate
Summarize
Create task
```

---

# 89. Composer Action

Example:

```text
Attach from integration
Insert template
Generate structured card
```

---

# 90. Message Context Action

Example:

```text
Save to notes
Translate
Create issue
```

---

# 91. File Action

Example:

```text
Open in plugin
Analyze
Upload to integration
```

subject to permissions.

---

# 92. Contact Action

Example:

```text
Create CRM record
```

if contact permission granted.

---

# 93. Search Provider

Plugin results show provider identity.

---

# 94. Search Provider Failure

Does not break global search.

---

# 95. Command Palette Integration

Plugin command includes:

```text
name
description
required context
capability
```

---

# 96. Command Execution

Always routes:

```text
UI
→ Plugin Command
→ Rust Plugin Runtime
```

---

# 97. No Direct UI-to-Plugin Process Bypass

Hard rule.

---

# 98. Composer Send Boundary

Plugin may create:

```text
draft content
structured attachment
```

but final send goes through core Composer/Send flow.

---

# 99. Message Write Capability

If plugin can initiate send:

```text
explicit user confirmation
```

recommended for v1.

---

# 100. Autonomous Sending

High-risk.

Defer unless product explicitly supports automations with strict policy.

---

# 101. Notifications

Plugin can request notification intent.

Core notification policy decides:

```text
privacy
urgency
display
```

---

# 102. Plugin Cannot Create Fake Security Alert

Reserved notification categories:

```text
Security
Emergency
IncomingCall
```

must be inaccessible to ordinary plugin.

---

# 103. Background Tasks

Plugin declares:

```text
task type
frequency class
resource need
network need
```

---

# 104. Background Scheduling

Rust scheduler owns.

Plugin does not create arbitrary OS alarms.

---

# 105. Android Background

Plugin background work constrained by app/service policy.

---

# 106. Desktop Background

Daemon can host plugin tasks if policy.

---

# 107. User Controls

Per plugin:

```text
Allow background activity
Allow on mobile data
Allow while battery saver
```

if needed.

---

# 108. Plugin Event Subscriptions

Capabilities determine events plugin can observe.

---

# 109. Event Minimization

Do not broadcast all app events to every plugin.

---

# 110. Example

Search plugin should not automatically receive:

```text
call events
device security events
```

---

# 111. Security Events

Reserved core namespace.

Plugins may not suppress/alter core security events.

---

# 112. Emergency Events

Reserved.

Plugins may only interact if explicit emergency extension capability exists.

Recommendation:

```text
no emergency plugin access in v1
```

---

# 113. Identity/Keys

Plugins never receive raw private key material.

Hard rule.

---

# 114. Device Authorization

Plugins cannot link/revoke devices directly.

Could request navigation to Security Center.

---

# 115. Backup/Restore

Plugins may contribute their own portable state.

---

# 116. Plugin Backup Scope

Each plugin declares:

```text
portable data
rebuildable cache
secret references
```

---

# 117. Plugin Restore

Plugin state restored only if:

```text
plugin compatible
manifest matches
migration succeeds
```

---

# 118. Missing Plugin on Restore

Keep plugin data dormant or mark:

```text
Plugin not installed
```

according to policy.

---

# 119. Plugin Data Export

User may export plugin data separately if supported.

---

# 120. Plugin Removal and Backup

Backup may retain plugin state until retention policy removes it.

---

# 121. Enterprise Policy

Managed deployments can define:

```text
Allowed plugins
Blocked plugins
Required plugins
Permission ceilings
Marketplace source restrictions
Auto-update policy
```

---

# 122. Required Plugin

UI shows:

```text
Required by organization
```

and disable/remove actions unavailable.

---

# 123. Blocked Plugin

Cannot install/enable.

---

# 124. Permission Ceiling

Even if plugin asks for NetworkAccess:

```text
organization policy may deny it
```

---

# 125. Effective Plugin Permission

```rust
pub struct EffectivePluginCapability {
    pub capability: PluginCapability,
    pub requested: bool,
    pub granted: bool,
    pub source: CapabilityGrantSource,
}
```

---

# 126. Grant Source

```rust
pub enum CapabilityGrantSource {
    User,
    OrganizationPolicy,
    SystemRestriction,
}
```

---

# 127. Permission Review Screen

Shows:

```text
Granted
Denied
Managed
```

---

# 128. Runtime Permission Request

If plugin asks for new scope:

```text
Plugin X wants access to selected files
Allow Once
Allow
Deny
```

where temporary grants are supported.

---

# 129. Temporary Grants

Useful for:

```text
file picker
specific conversation
one-time export
```

---

# 130. Persistent Grant

Should be explicit.

---

# 131. Permission Revocation

User can revoke capability.

Plugin receives:

```text
CapabilityRevoked
```

and must degrade safely.

---

# 132. Plugin UX After Revocation

Example:

```text
File access disabled
Grant Access
```

---

# 133. Permission Revocation Does Not Uninstall Plugin

Separate.

---

# 134. Plugin Status Screen

Show:

```text
Enabled
Disabled
Crashed
Quarantined
Needs Permission
Update Available
```

---

# 135. Plugin Badges

Keep small and semantic.

---

# 136. Installed Screen — Desktop

Recommended:

```text
Left list
Right details/settings
```

---

# 137. Installed Screen — Android

List → plugin detail destination.

---

# 138. Plugin Detail Actions

```text
Enable/Disable
Update
Permissions
Settings
Storage
Diagnostics
Remove
```

---

# 139. Plugin Discover — Desktop

Grid/list.

Search/filter by:

```text
category
publisher
permission level
platform
```

---

# 140. Plugin Discover — Android

Cards/list with concise permission summary.

---

# 141. Marketplace Search

Separate from private message search.

---

# 142. Marketplace Privacy

Do not send private app state to marketplace search.

---

# 143. Reviews/Ratings

Optional future.

Not a security signal.

---

# 144. Recommended Plugins

If personalized, use privacy-safe local logic or explicit server-side consent.

---

# 145. Plugin Detail Changelog

Useful for permission changes.

---

# 146. Update Permission Diff

Show:

```text
New:
- Network access

Removed:
- Contacts access
```

---

# 147. Update Deferral

User can stay on old version if still safe.

---

# 148. Security Revocation Overrides Deferral

If plugin/package revoked:

```text
disable immediately
```

---

# 149. Plugin API Deprecation

Show:

```text
This plugin may stop working in a future version
```

---

# 150. Compatibility Failure

Do not crash app.

---

# 151. Plugin Diagnostics

Show:

```text
runtime state
last error
resource usage
version
capabilities
```

---

# 152. Diagnostics Privacy

No plugin secrets/raw message content by default.

---

# 153. Export Plugin Diagnostics

Redacted support bundle.

---

# 154. Developer Mode

Unlock:

```text
Install local package
Load workspace plugin
Hot reload
Verbose plugin logs
Capability inspector
```

---

# 155. Developer Warning

```text
Developer plugins may be unsafe and can access data according to granted permissions.
```

---

# 156. Local Development Badge

Clearly show:

```text
Developer
```

---

# 157. Hot Reload

Desktop-focused feature.

Android may require rebuild/package install depending runtime.

---

# 158. Plugin Manifest View

Developer details:

```text
PluginId
version
API version
capabilities
extension points
entrypoints
```

---

# 159. Plugin Permission Test Mode

Developer tool can simulate:

```text
permission denied
network unavailable
storage quota exceeded
```

---

# 160. Plugin Sandbox Violation

Runtime blocks.

UI can show:

```text
Plugin action blocked by security policy
```

---

# 161. Repeated Sandbox Violation

May auto-disable/quarantine.

---

# 162. Resource Quota Violation

```text
Plugin paused because it exceeded resource limits
```

---

# 163. User Recovery

Actions:

```text
Restart
Disable
Reset plugin data
Remove
```

---

# 164. Plugin Data Migration Failure

On update:

```text
Update could not be completed
Old version restored
```

---

# 165. Rollback

Important for resilient updates.

---

# 166. Plugin Settings Corruption

Reset only plugin config.

Do not affect core settings.

---

# 167. Plugin Search Integration

Part 11:

```text
global search
→ plugin provider result
```

Each result identifies plugin.

---

# 168. Plugin Composer Integration

Part 06:

```text
declarative composer action
```

No raw editor internals.

---

# 169. Plugin File Integration

Part 10:

```text
file action
```

receives scoped file handle.

---

# 170. Plugin Contact Integration

Part 08:

```text
contact action
```

only with permission.

---

# 171. Plugin Group Integration

Part 09:

```text
group action
```

only with permission.

---

# 172. Plugin Notification Integration

Part 13 core policy remains authoritative.

---

# 173. Plugin Settings Integration

Part 18 provides global permissions/settings entry.

---

# 174. Plugin Security Integration

Part 15 shows plugin-related critical events if security-relevant.

---

# 175. Plugin Backup Integration

Part 16 can include portable plugin state.

---

# 176. Plugin Offline Behavior

Plugin must declare whether it works:

```text
offline
local-only
requires Internet
```

---

# 177. Offline UX

Show:

```text
Requires Internet
```

rather than generic failure.

---

# 178. Local-Only Plugin

Can continue offline.

---

# 179. Network Failure

Plugin UI remains contained.

---

# 180. Accessibility — Plugin List

Screen reader:

```text
Translate Plugin, enabled, verified publisher, update available
```

---

# 181. Permission Accessibility

Example:

```text
Read messages, granted
Network access, denied
```

---

# 182. Risk Explanation Accessibility

Text-based.

Never rely on shield color.

---

# 183. Large Font

Permission descriptions wrap.

---

# 184. RTL

Plugin UI mirrors.

Plugin names remain source text.

---

# 185. Reduced Motion

Install/update progress not dependent on animation.

---

# 186. Keyboard Desktop

Support:

```text
Ctrl/Cmd+F search plugins
Enter open
Space enable/disable where safe
Shift+F10 context menu
```

---

# 187. Android TalkBack

All install/permission/update actions have explicit semantics.

---

# 188. Plugin-Provided UI Accessibility

Declarative extension schema must require:

```text
labels
roles
states
```

---

# 189. Reject Inaccessible Extension

Developer validation should flag missing labels.

---

# 190. Plugin UI Theme

Uses host tokens.

---

# 191. No Arbitrary Styling Override

Plugin should not break global typography/contrast.

---

# 192. Plugin Icons

Use safe bounded assets.

---

# 193. Plugin Image/Asset Limits

Size/format bounds.

---

# 194. Plugin Text

Treat as untrusted.

No raw HTML injection.

---

# 195. External Links

Open through validated host action.

---

# 196. Plugin Web Content

Avoid embedded arbitrary webview by default.

---

# 197. Android Platform Restriction

Pure Compose host should not require plugin-provided Android Activities unless extension architecture explicitly supports them.

---

# 198. Desktop Native Extension

Prefer host-declarative panels instead of arbitrary native windows.

---

# 199. Plugin Commands in Context Menu

Show only when:

```text
context applicable
capability granted
plugin healthy
```

---

# 200. Plugin Action Timeout

If plugin stalls:

```text
Action timed out
```

core UI remains responsive.

---

# 201. Plugin Confirmation

High-impact plugin action should request user confirmation.

Example:

```text
Send selected messages to external service?
```

---

# 202. External Data Transfer Disclosure

If plugin can combine:

```text
ReadMessages + NetworkAccess
```

show clear disclosure before first use.

---

# 203. Data Egress Prompt

Potential:

```text
This action will send selected message content to example.com
Continue
```

---

# 204. Allow Always

Only where safe and understandable.

---

# 205. Secret Provider Credentials

Plugin gets opaque credential handle, not raw secret if architecture supports brokered requests.

---

# 206. Brokered Network Requests

Preferred for sensitive integrations.

---

# 207. Plugin Permission Audit

Security Center/Plugin detail can show:

```text
what permissions were used recently
```

optional future.

---

# 208. No Surveillance-Like Audit by Default

Avoid over-collecting user behavior.

---

# 209. Plugin Install Source History

Can record:

```text
Marketplace
Local file
Organization
```

for security diagnostics.

---

# 210. Plugin Provenance

Useful in backup/restore and incident review.

---

# 211. Plugin Event Model

```rust
pub enum PluginUiEvent {
    Installed(PluginSummaryView),
    Removed(PluginId),
    StateChanged {
        plugin: PluginId,
        state: PluginInstallState,
    },
    PermissionChanged {
        plugin: PluginId,
        capability: PluginCapability,
    },
    UpdateAvailable(PluginUpdateView),
    RuntimeHealthChanged {
        plugin: PluginId,
        health: PluginRuntimeHealth,
    },
}
```

---

# 212. Plugin Presentation API

```rust
pub trait PluginPresentation {
    async fn installed(
        &self,
    ) -> Result<Vec<PluginSummaryView>, UiError>;

    async fn details(
        &self,
        plugin: PluginId,
    ) -> Result<PluginDetailView, UiError>;

    async fn install(
        &self,
        source: PluginInstallSource,
    ) -> Result<PluginInstallJobView, UiError>;

    async fn enable(
        &self,
        plugin: PluginId,
    ) -> Result<(), UiError>;

    async fn disable(
        &self,
        plugin: PluginId,
    ) -> Result<(), UiError>;

    async fn remove(
        &self,
        plugin: PluginId,
        data_policy: PluginDataRemovalPolicy,
    ) -> Result<(), UiError>;
}
```

---

# 213. Permission Presentation API

```rust
pub trait PluginPermissionPresentation {
    async fn permissions(
        &self,
        plugin: PluginId,
    ) -> Result<Vec<PluginPermissionView>, UiError>;

    async fn grant(
        &self,
        plugin: PluginId,
        grant: PluginCapabilityGrant,
    ) -> Result<(), UiError>;

    async fn revoke(
        &self,
        plugin: PluginId,
        capability: PluginCapability,
    ) -> Result<(), UiError>;
}
```

---

# 214. Update Presentation API

```rust
pub trait PluginUpdatePresentation {
    async fn updates(
        &self,
    ) -> Result<Vec<PluginUpdateView>, UiError>;

    async fn update(
        &self,
        plugin: PluginId,
    ) -> Result<PluginInstallJobView, UiError>;

    async fn rollback(
        &self,
        plugin: PluginId,
    ) -> Result<(), UiError>;
}
```

---

# 215. Extension Registry API

```rust
pub trait PluginExtensionPresentation {
    async fn extensions(
        &self,
        point: UiExtensionPoint,
        context: ExtensionContext,
    ) -> Result<Vec<UiExtensionView>, UiError>;

    async fn invoke(
        &self,
        extension: ExtensionId,
        context: ExtensionContext,
    ) -> Result<ExtensionInvocationResult, UiError>;
}
```

---

# 216. Plugin Settings API

```rust
pub trait PluginSettingsPresentation {
    async fn schema(
        &self,
        plugin: PluginId,
    ) -> Result<PluginSettingsSchemaView, UiError>;

    async fn update(
        &self,
        plugin: PluginId,
        update: PluginSettingsUpdate,
    ) -> Result<(), UiError>;
}
```

---

# 217. Plugin Install Job

```rust
pub enum PluginInstallPhase {
    Downloading,
    Verifying,
    CheckingCompatibility,
    Installing,
    MigratingData,
    Finalizing,
}
```

---

# 218. Install Progress

Show semantic phase.

Do not expose package extraction internals.

---

# 219. Android ViewModel

Owns:

```text
category/search
permission sheets
install confirmation
system/file-picker effects
plugin settings form state
```

Rust owns plugin lifecycle/trust.

---

# 220. Dioxus Presenter

Owns:

```text
list/detail selection
developer package picker
settings panels
command/extension presentation
```

---

# 221. No Plugin State in UI Persistence

UI reloads from Rust.

---

# 222. No Plugin Can Mutate Core UI State Directly

All extension actions go through registered APIs.

---

# 223. Installation from File — Android

Use document picker.

Pass handle/FD to Rust.

---

# 224. Installation from File — Desktop

Native file picker.

Rust validates package.

---

# 225. No Raw Package Bytes Through JNI

Hard rule.

---

# 226. Marketplace Download

Rust/plugin service owns verified download/install pipeline.

---

# 227. Offline Plugin Install

Local package can install offline if valid.

---

# 228. Marketplace Offline

Show:

```text
Internet required
```

---

# 229. Plugin License

Detail screen may show:

```text
license
source code link
```

if metadata exists.

---

# 230. Open-Source Badge

Informational only.

Not security proof.

---

# 231. Privacy Policy

Plugin detail can link publisher privacy policy if network/data access exists.

---

# 232. Permission Diff History

Security event can record major permission escalation.

---

# 233. Security Notification

If plugin suddenly quarantined:

```text
Plugin disabled for security reasons
```

Part 13 Security category.

---

# 234. Core Product Survival

If plugin subsystem fails:

```text
messaging
calls
security
files
```

must continue.

---

# 235. Safe Mode

Startup option:

```text
Start without plugins
```

if crash loop suspected.

---

# 236. Safe Mode UX

Show:

```text
Plugins are temporarily disabled
```

with:

```text
Review Plugins
Restart Normally
```

---

# 237. Crash Loop Detection

Rust/runtime owns.

---

# 238. Plugin Recovery

After app crash caused by plugin:

```text
plugin auto-disabled
```

if confidently attributed.

---

# 239. Plugin Data Corruption

Isolate to plugin namespace.

---

# 240. Storage Quota

If exceeded:

```text
Plugin storage full
Manage
```

---

# 241. Network Quota

Optional.

---

# 242. Background Task Failure

Plugin detail shows:

```text
Last background task failed
```

only if user cares.

---

# 243. Plugin Notification Noise

Core notification policy can rate-limit plugin category.

---

# 244. Plugin UI Noise

Limit number of:

```text
sidebar entries
toolbar icons
context actions
```

---

# 245. Extension Overflow

Use:

```text
More actions
```

when many plugin actions exist.

---

# 246. Priority

Core actions always precede plugin actions.

---

# 247. Visual Separation

Plugin action may show:

```text
plugin icon/name
```

in overflow menus.

---

# 248. Plugin Result Attribution

Search/composer/file actions identify source plugin.

---

# 249. Trust at Point of Use

If high-risk plugin acts on sensitive content, show attribution:

```text
Send to Translator Plugin
```

not generic:

```text
Translate
```

if action transmits data externally.

---

# 250. Accessibility Test Matrix

Verify:

```text
install
permission review
enable/disable
update
remove
runtime failure
plugin settings
extension action
```

with screen reader/keyboard.

---

# 251. Android Tests

Verify:

```text
document picker local install
process death
background task permission
TalkBack
large font
permission sheets
```

---

# 252. Desktop Tests

Verify:

```text
two-pane plugin manager
keyboard
local package install
developer mode
safe mode
command palette extensions
```

---

# 253. Security Tests

Required:

```text
unsigned plugin
bad signature
revoked package
permission escalation
network+messages
sandbox escape attempt
private-key access attempt
reserved notification spoof
```

---

# 254. Crash Tests

```text
plugin panic
infinite loop
memory overuse
background crash
migration failure
```

Core app survives.

---

# 255. Update Tests

```text
normal update
new permission
bad signature
migration failure
rollback
revoked old version
```

---

# 256. Restore Tests

```text
plugin installed
plugin missing
plugin incompatible
plugin state migration
secret reference missing
```

---

# 257. Multi-Device Plugins

Installation policy can be:

```text
device-local
```

by default.

---

# 258. Account-Wide Plugin Preference

Optional:

```text
remember installed plugin list
```

but each device still validates platform compatibility and permissions.

---

# 259. Permission Sync

Recommendation:

```text
do not blindly sync sensitive grants
```

Each device can require local grant confirmation.

---

# 260. Plugin Settings Sync

Only semantic, non-secret settings if product chooses.

---

# 261. Android/Desktop Difference

A desktop-only plugin can remain remembered but not installed on Android.

---

# 262. Enterprise Required Plugin

Can auto-install only from trusted organization source and within policy.

User still sees permissions/effective policy.

---

# 263. Initial Production Scope

Ship:

```text
installed plugins
local/verified catalog install
enable/disable/remove
signed manifest verification
capability permissions
permission review
plugin settings schema
command/search/composer/file extensions
sandbox/resource limits
update/rollback
crash isolation
safe mode
developer local install
organization allow/block policy
```

Defer:

```text
arbitrary native UI injection
unrestricted background automation
plugin-to-plugin arbitrary IPC
full browser/webview plugins
unbounded marketplace social features
emergency/security authority extensions
```

---

# 264. Definition of Done

UI/UX Part 19 is complete when:

- plugins are stable `PluginId`-identified entities
- install, enable, disable, update, quarantine, and removal states are explicit
- publisher/signature/provenance is visible but not confused with harmlessness
- capability permissions are narrow, typed, and reviewable before grant
- permission escalation on update requires review
- message/file/network/background capabilities are separate
- plugin data storage is isolated and removable independently
- plugins cannot access raw identity/private-key/device-authorization authority
- plugin notifications go through core notification policy
- plugin composer actions cannot bypass core message-send semantics
- extension points are declarative and bounded
- crashes, loops, memory abuse, and sandbox violations cannot crash the core app
- repeated failures can auto-disable/quarantine a plugin
- signed updates are atomic and rollback-capable
- organization policy can allow/block/require plugins and cap permissions
- local/developer installs are visually distinct from catalog installs
- Android/desktop file-install flows use handles rather than giant byte arrays
- accessibility, attribution, RTL, large font, keyboard/TalkBack, and safe-mode behavior are explicit
- Rust plugin lifecycle, permission, update, settings, and extension presentation APIs are specified
- signature, permission, sandbox, crash, update, restore, and multi-device tests are included

---

# 265. Final Architecture

```text
                       PLUGIN PACKAGE
                            │
                            ▼
                   Rust Plugin Runtime
                            │
       ┌────────────────────┼────────────────────┐
       │                    │                    │
    Trust               Capabilities          Sandbox
       │                    │                    │
 Signature            Permission Grants    Resource Limits
 Provenance            Extension Points     Crash Isolation
       │                    │                    │
       └────────────────────┼────────────────────┘
                            │
                   Plugin Presentation
                     ┌──────┴──────┐
                     │             │
                  Dioxus        Compose
                     │             │
             Desktop Plugins   Android Plugins
```

Core boundary:

```text
Plugin
→ declared capability
→ Rust broker
→ core service
```

Never:

```text
Plugin
→ raw database / keys / transport / OS authority
```

---

# 266. Final Principle

A healthy plugin ecosystem gives third-party code useful capability without giving it implicit trust.

The correct model is:

```text
signed identity
+
narrow capabilities
+
explicit consent
+
sandboxed execution
+
bounded UI extension points
+
core-owned security
```

not:

```text
install plugin
→ plugin can do anything the app can do
```

This gives Dioxus desktop and Android Compose an extensible ecosystem while preserving the Rust core as the only authority over identity, messaging, security, storage, notifications, and device trust.
