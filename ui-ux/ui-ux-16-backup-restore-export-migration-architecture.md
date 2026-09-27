# UI/UX Part 16 — Backup, Restore, Export & Migration UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 16  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete backup, restore, export, import, archival, device migration, recovery verification, progress, failure, storage-provider, portability, and Rust presentation architecture across desktop and Android.

---

# 1. Purpose

Backup and migration are not ordinary file operations.

They are security-sensitive lifecycle operations that can determine whether a user can recover:

```text
account identity
contacts
conversation history
attachments
settings
verification state
device-independent metadata
```

after:

```text
device loss
device replacement
app reinstall
storage corruption
migration to a new machine
long-term archival
```

The governing principle is:

> **The UI should make recoverability understandable while Rust owns backup format, encryption, integrity, migration compatibility, restore safety, and conflict policy.**

---

# 2. Architectural Position

```text
Rust Backup / Restore Core
        │
        ├── snapshot planning
        ├── encryption
        ├── versioning
        ├── migration
        ├── integrity
        ├── selective restore
        ├── export/import
        └── archive semantics
        │
        ▼
Backup Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

Platform adapters own only:

```text
file picker
folder picker
SAF/document provider
filesystem destination
share/export UI
foreground progress surfaces
```

---

# 3. Important Conceptual Separation

The UI must distinguish:

```text
Backup
Restore
Export
Import
Migration
Archive
```

These are not synonyms.

---

# 4. Backup

Purpose:

```text
create a recoverable protected copy
```

of selected logical user data.

---

# 5. Restore

Purpose:

```text
recover logical state from a backup
```

into a safe current runtime.

---

# 6. Export

Purpose:

```text
produce portable user-readable/interoperable data
```

Examples:

```text
JSON/CSV/Markdown
media folder
conversation archive
```

depending implementation.

Export may not contain everything needed for full recovery.

---

# 7. Import

Purpose:

```text
ingest supported external or previous-app data
```

with provenance and validation.

---

# 8. Migration

Purpose:

```text
move account/application state to a new device/runtime
```

possibly using backup/restore internally.

---

# 9. Archive

Purpose:

```text
long-term durable storage
```

with emphasis on future readability rather than immediate app operation.

---

# 10. Backup Classes

Recommended logical classes:

```rust
pub enum BackupDataClass {
    Authoritative,
    Rebuildable,
    Ephemeral,
    HardwareBound,
    Secret,
}
```

---

# 11. Authoritative

Examples:

```text
messages
contacts
group metadata
user settings
local durable drafts if included
```

---

# 12. Rebuildable

Examples:

```text
search indexes
thumbnails
derived caches
```

Usually excluded.

---

# 13. Ephemeral

Examples:

```text
typing
presence
call transient state
```

Excluded.

---

# 14. Hardware-Bound

Examples:

```text
device keystore handles
biometric enrollment
platform-specific tokens
```

Cannot be blindly restored.

---

# 15. Secret

Examples:

```text
recovery material
backup key envelopes
```

Handled through explicit security policy.

---

# 16. Backup Overview Screen

Recommended:

```text
Backup status
Last backup
Next/automatic backup if configured
Destination
Recovery readiness
Backup size
Create Backup
Restore
Export Data
```

---

# 17. Backup Status

```rust
pub enum BackupStatus {
    NeverCreated,
    Healthy,
    Running,
    Stale,
    Failed,
    NeedsAttention,
}
```

---

# 18. Healthy

Example:

```text
Last backup: yesterday
Verified
```

---

# 19. Stale

Example:

```text
Last backup: 45 days ago
```

with:

```text
Back Up Now
```

---

# 20. Failed

Example:

```text
Backup failed
Storage unavailable
Retry
```

---

# 21. Backup Destination Types

Potential:

```rust
pub enum BackupDestinationKind {
    LocalFile,
    LocalFolder,
    RemovableStorage,
    UserSelectedProvider,
    TrustedArchive,
}
```

---

# 22. Provider Neutrality

UI should not hardcode backend architecture to one cloud provider.

---

# 23. Android Destination

Use:

```text
Storage Access Framework
DocumentProvider
CreateDocument
OpenDocumentTree
```

rather than broad filesystem permissions.

---

# 24. Desktop Destination

Use native:

```text
Save File
Select Folder
```

depending container format.

---

# 25. Backup Container

Normal UI should present:

```text
Encrypted backup
```

not internal chunk/container layout.

---

# 26. Backup Format Version

Advanced detail:

```text
Backup format v3
```

only if useful for support.

---

# 27. Backup Creation Flow

Recommended:

```text
Backup
→ Choose destination
→ Choose scope
→ Confirm recovery method
→ Estimate size
→ Create
→ Verify
→ Complete
```

---

# 28. Backup Scope

Potential:

```text
Full Backup
Messages only
Messages + Files
Account metadata only
```

Recommendation:

```text
Full Backup
Custom
```

for simpler UX.

---

# 29. Full Backup

Includes all portable authoritative data allowed by policy.

---

# 30. Custom Backup

Advanced screen selects:

```text
messages
attachments
contacts
groups
settings
drafts
security metadata
```

according to backend support.

---

# 31. Exclusion Disclosure

Tell user if backup excludes:

```text
search indexes
temporary downloads
hardware-bound credentials
live call state
```

but do not overwhelm.

---

# 32. Backup Size Estimate

Rust computes estimate.

UI shows:

```text
About 4.8 GB
```

not fake exactness before final packaging.

---

# 33. Destination Free Space

Platform may report if available.

---

# 34. Insufficient Space

Show:

```text
Not enough space
Choose another destination
```

---

# 35. Backup Encryption

Backup is encrypted by default.

No unencrypted full backup in normal UX.

---

# 36. Recovery Key Relationship

Backup may use:

```text
dedicated backup key
recipient envelopes
account recovery method
```

The UI must accurately describe whichever design is implemented.

---

# 37. Account Recovery vs Backup Recovery

Hard distinction if separate.

Show:

```text
Account Recovery
Backup Recovery
```

not one ambiguous "Recovery".

---

# 38. Backup Recovery Key

If distinct, label:

```text
Backup Recovery Key
```

---

# 39. Sensitive Key Display

Use Part 15 sensitive screen rules:

```text
reauth
show/hide
copy
save
verify
```

---

# 40. Backup Key Verification

Before backup considered recoverable:

```text
verify recovery method
```

where applicable.

---

# 41. Backup Job Model

```rust
pub struct BackupJobView {
    pub id: BackupJobId,
    pub state: BackupJobState,
    pub phase: BackupPhase,
    pub progress: BackupProgressView,
    pub destination: BackupDestinationView,
    pub started_at: Timestamp,
}
```

---

# 42. Backup Job State

```rust
pub enum BackupJobState {
    Planned,
    Running,
    Paused,
    Verifying,
    Complete,
    Failed,
    Cancelled,
}
```

---

# 43. Backup Phases

```rust
pub enum BackupPhase {
    Planning,
    Snapshotting,
    Packing,
    Encrypting,
    Writing,
    Verifying,
    Finalizing,
}
```

---

# 44. User-Facing Phase Mapping

Normal UI may simplify to:

```text
Preparing
Backing up
Verifying
Finishing
```

---

# 45. Backup Progress

Show:

```text
percentage if meaningful
bytes processed
items processed
time remaining only if stable
```

---

# 46. Do Not Fake Linear Progress

If total unknown:

```text
indeterminate phase
```

is better.

---

# 47. Backup Cancellation

Allowed if Rust can clean up safely.

---

# 48. Cancel Confirmation

For long backup:

```text
Cancel backup?
Partial temporary data will be removed.
```

---

# 49. Crash Safety

If app crashes:

```text
incomplete backup must not look valid
```

---

# 50. Atomic Finalization

Only after full verification should destination be considered:

```text
Complete
```

---

# 51. Backup Verification

Mandatory integrity phase.

---

# 52. Verification Result

```rust
pub enum BackupVerificationState {
    Verified,
    Unverified,
    Failed,
}
```

---

# 53. Verified

Means:

```text
container structure
integrity/authentication
required metadata
```

passed according to backend.

---

# 54. Restore Overview

Entry points:

```text
Restore Backup
Recover on New Device
Import Data
```

---

# 55. Restore Is High Risk

Restore can overwrite/merge logical state.

Require planning before commit.

---

# 56. Restore Flow

Recommended:

```text
Select backup
→ Inspect/verify
→ Authenticate/decrypt
→ Analyze compatibility
→ Build restore plan
→ Dry-run summary
→ Choose scope
→ Confirm
→ Restore
→ Verify
→ Rebuild derived state
```

---

# 57. Restore Inspection

Before mutation show:

```text
backup date
backup version
account identity
size
data classes
source device
verification status
```

where available.

---

# 58. Wrong Backup

If backup belongs to another account:

```text
This backup belongs to a different identity
```

Do not continue silently.

---

# 59. Backup Version Compatibility

```rust
pub enum BackupCompatibility {
    Compatible,
    MigrationRequired,
    ReadOnlyArchiveOnly,
    UnsupportedNewer,
    Corrupt,
}
```

---

# 60. Compatible

Normal restore.

---

# 61. Migration Required

Rust can migrate old format.

UI:

```text
This backup will be upgraded during restore
```

---

# 62. Unsupported Newer

```text
This backup was created by a newer version of the app
Update the app
```

---

# 63. Archive-Only

Can be opened/read/exported but not restored into active state.

---

# 64. Corrupt

Do not restore.

Offer:

```text
Try another backup
Diagnostics
```

---

# 65. Restore Plan

```rust
pub struct RestorePlanView {
    pub plan_id: RestorePlanId,
    pub compatibility: BackupCompatibility,
    pub scope: RestoreScopeView,
    pub conflicts: Vec<RestoreConflictView>,
    pub warnings: Vec<RestoreWarningView>,
    pub estimated_changes: RestoreChangeSummary,
}
```

---

# 66. Dry Run

Restore planning should be non-mutating.

---

# 67. Dry-Run Summary

Example:

```text
12 conversations will be restored
2,430 messages added
120 files restored
3 existing items merged
1 security-sensitive state skipped
```

---

# 68. Selective Restore

Potential:

```text
Account Metadata
Contacts
Conversations
Files
Settings
Drafts
```

---

# 69. Default Restore

Recommended:

```text
safe full logical restore
```

with security-invalid classes automatically excluded.

---

# 70. Never Blindly Restore

Do not restore stale:

```text
live sessions
ratchet state
device authorization tokens
platform keystore handles
```

into a new device.

---

# 71. New Device Restore

New device gets fresh:

```text
DeviceId
device keys
session state
```

while portable user data is restored.

---

# 72. Restore Security Warning

Explain:

```text
This backup restores your data, but this device will use fresh device security keys.
```

---

# 73. Existing Device Restore

May require merge with current data.

---

# 74. Restore Modes

Potential:

```rust
pub enum RestoreMode {
    Merge,
    ReplaceLocalPortableData,
    NewDeviceRecovery,
}
```

---

# 75. Merge

Recommended default for an existing active account.

---

# 76. Replace Local Portable Data

Advanced/destructive.

Does not replace account identity/security unless explicitly designed.

---

# 77. Conflict Types

```rust
pub enum RestoreConflictKind {
    SameLogicalItemDifferentVersion,
    LocalOnlyNewer,
    BackupOnly,
    IdentityMismatch,
    UnsupportedState,
}
```

---

# 78. Conflict Resolution

Rust should auto-resolve using deterministic domain rules where safe.

UI only asks user when semantics truly require a choice.

---

# 79. Avoid Per-Message Conflict Dialogs

Hard rule.

---

# 80. Provenance

Imported/restored data can retain:

```text
restored from backup
imported from file
```

in metadata/audit where useful.

---

# 81. Restore Progress

Phases:

```text
Decrypting
Validating
Migrating
Applying
Verifying
Rebuilding search
```

---

# 82. Search Rebuild

Part 11 search index can rebuild after core data restore.

---

# 83. Derived State

Rebuild:

```text
search indexes
thumbnails
caches
```

rather than restoring stale copies.

---

# 84. Restore Completion

Show:

```text
Restore complete
```

with summary.

---

# 85. Restore Partial Failure

If transaction architecture allows rollback:

```text
rollback
```

and report failure.

If partial commits can occur, UI must say exactly what succeeded.

Recommendation:

```text
transactional/staged restore
```

so active state flips only after validation.

---

# 86. Restore Cancellation

Before commit:

```text
safe
```

During commit:

```text
only if backend supports transactional cancellation
```

otherwise disable Cancel and explain finishing safely.

---

# 87. Restore Verification

After applying:

```text
validate resulting state
```

before declaring success.

---

# 88. Migration to New Device

User-facing migration flow can be simpler than manual backup.

---

# 89. Device Migration Flow

Recommended:

```text
Old device:
    Devices
    → Move to New Device

New device:
    Scan QR / enter migration code

Both:
    authenticate
    choose transfer scope
    transfer encrypted data
    verify
    authorize new device
    optionally revoke old device
```

---

# 90. Migration vs Device Link

Device link:

```text
adds another device
```

Migration:

```text
moves primary usage/data and may retire old device
```

---

# 91. Migration Uses Part 12 Trust

Bootstrap/security uses device-link architecture.

---

# 92. Migration Data Plane

Can use:

```text
LAN/QUIC
direct cable/network
encrypted backup handoff
```

backend choice.

UI stays transport-neutral.

---

# 93. Migration Scope

Potential:

```text
All data
Messages only
Without large media
```

---

# 94. Migration Progress

Show meaningful:

```text
Conversations
Files
Settings
```

without raw chunk count.

---

# 95. Old Device Retention

At end ask:

```text
Keep this device linked
Revoke old device
```

if allowed.

---

# 96. Do Not Auto-Revoke Before Success

Hard rule.

---

# 97. Migration Failure

Old device remains authoritative/trusted unless security policy says otherwise.

---

# 98. Migration Resume

If direct transfer interrupted and backend supports checkpoints:

```text
Resume
```

---

# 99. Export UX

Separate screen:

```text
Export Data
```

---

# 100. Export Formats

Potential:

```rust
pub enum ExportFormat {
    PortableArchive,
    Markdown,
    Json,
    Csv,
    MediaFolder,
}
```

Only expose implemented formats.

---

# 101. Portable Archive

Human-preserving but not necessarily app-restorable.

---

# 102. Markdown Export

Good for:

```text
conversation transcripts
```

---

# 103. JSON

Good for machine portability.

---

# 104. CSV

Limited structured tables only.

---

# 105. Media Folder

Exports attachments to normal files.

---

# 106. Export Scope

Potential:

```text
All conversations
Selected conversation
Date range
Selected files
Contacts
```

---

# 107. Export Privacy Warning

Exports may be:

```text
unencrypted
```

depending format.

Must warn explicitly.

---

# 108. Encrypted Export

Portable archive may support encrypted mode.

---

# 109. Export Destination Android

Use SAF/document provider.

---

# 110. Export Destination Desktop

Native save/folder picker.

---

# 111. Export Filename

Safe sanitized default.

---

# 112. Export Provenance

Include metadata:

```text
export date
app version
format version
```

where useful.

---

# 113. Export Does Not Change App Data

Hard rule.

---

# 114. Import UX

Separate:

```text
Import Data
```

---

# 115. Import Sources

Potential:

```text
previous app export
portable archive
supported message format
contacts file
```

---

# 116. Import Inspection

Before commit:

```text
source type
record count
date range
warnings
unsupported content
```

---

# 117. Import Provenance

Imported data should carry origin where relevant.

---

# 118. Import Trust

Imported messages/files are not automatically:

```text
cryptographically verified
```

as if received live.

---

# 119. Imported Identity

Do not create verified contacts solely from imported display names.

---

# 120. Import Conflict Handling

Same deterministic approach as restore.

---

# 121. Import Dry Run

Recommended.

---

# 122. Unsupported Records

Show aggregate:

```text
12 items could not be imported
View details
```

---

# 123. Archive UX

Long-term archive differs from backup.

---

# 124. Archive Goals

```text
future readability
stable metadata
media preservation
integrity
```

---

# 125. Archive View

Could allow:

```text
Open Archive
Browse Conversations
Search Archive
Export from Archive
```

without restoring into active account.

---

# 126. Read-Only Archive

Strong recommendation.

---

# 127. Archive Reader

Uses safe read-only path.

---

# 128. Archive Search

Optional local index built for archive.

---

# 129. Archive Version Migration

Reader may support older versions without mutating original.

---

# 130. Backup Scheduling

If product supports automatic backups:

```text
Daily
Weekly
Manual only
```

---

# 131. Android Scheduling

Background constraints may make exact timing approximate.

UI should not promise exact minute unless architecture can.

---

# 132. Desktop Scheduling

Could run through daemon.

---

# 133. Automatic Backup Destination

Must remain accessible.

---

# 134. Destination Lost

Example:

```text
External drive not connected
```

Show:

```text
Backup delayed
```

not immediate destructive failure.

---

# 135. Provider Authentication Expired

If destination provider needs credentials:

```text
Reconnect storage
```

through platform/plugin connector.

---

# 136. Backup Retention

Potential:

```text
Keep last 3
Keep 30 days
Keep monthly archives
```

---

# 137. Retention UX

Show estimated storage effect.

---

# 138. Never Delete Only Good Backup Blindly

Retention engine should preserve at least one verified recovery point according to policy.

---

# 139. Backup History

Show:

```text
date
size
destination
verified
status
```

---

# 140. Backup Detail

Actions:

```text
Verify
Restore
Delete
Export/Move
```

depending destination permissions.

---

# 141. Delete Backup

High-impact.

Explain:

```text
This removes this recovery point.
```

---

# 142. Delete Last Backup

Warn strongly if no other verified backup exists.

---

# 143. Remote Provider Delete

Do not claim deletion if provider operation fails.

---

# 144. Backup Verification Drill

Action:

```text
Verify Backup
```

rechecks integrity without restore.

---

# 145. Recovery Drill

Part 15 can invoke backup verification.

---

# 146. Backup Age Warning

Configurable threshold.

---

# 147. Backup Notifications

Part 13.

Useful:

```text
backup failed
backup needs attention
manual backup completed
```

---

# 148. Routine Success

May remain silent.

---

# 149. Storage Management Integration

Part 10 can show:

```text
backup cache
temporary staging
```

not external backup files unless explicitly tracked.

---

# 150. Backup Staging

Temporary local staging must be cleanup-safe.

---

# 151. Low Storage

If local staging insufficient:

```text
Not enough temporary storage
```

---

# 152. Streaming Backup

Preferred for large data.

UI should not imply entire archive loads into memory.

---

# 153. Large Backup

Multi-gigabyte backup remains memory-bounded.

---

# 154. Progress Update Rate

Throttle to reasonable rate.

---

# 155. Background Backup — Android

If user-visible long-running backup requires foreground service:

```text
Backup in progress
```

notification may be required.

---

# 156. Background Backup — Desktop

Can continue in daemon mode.

---

# 157. App Close During Backup

Embedded mode:

```text
ask whether to cancel/quit
```

or wait for safe checkpoint.

Daemon mode:

```text
UI can close; backup continues
```

---

# 158. Power Loss

Incomplete destination must not be marked valid.

---

# 159. Backup Metadata

Normal UI may show:

```text
created
verified
app version
device
```

---

# 160. Security Metadata

Do not expose:

```text
key IDs
recipient envelopes
```

in normal UI.

---

# 161. Backup Destination Privacy

A filename may reveal identity.

Use neutral default names.

---

# 162. Example Backup Filename

```text
communication-backup-2026-08-25.backup
```

rather than contact names.

---

# 163. Export Filename

Conversation export may include group/contact name if user explicitly chooses and platform sanitizes.

---

# 164. Sensitive Export

If export contains plaintext:

```text
This export is not encrypted
```

must be visible before creation.

---

# 165. Android Recent Apps Privacy

Restore/recovery-key screens can use secure-window policy.

---

# 166. Desktop Sensitive Restore

Hide secrets after screen loses focus if policy.

---

# 167. Clipboard

Recovery key only explicit copy.

Backup path is not secret by itself but may be sensitive.

---

# 168. Backup Key Logging

Never log.

---

# 169. Restore Key Logging

Never log.

---

# 170. Export Content Logging

Never log content.

---

# 171. Safe Metrics

Possible:

```text
backup duration
backup bytes bucket
restore duration
failure category
format migration version
```

without content/path/secrets.

---

# 172. Backup Presentation API

```rust
pub trait BackupPresentation {
    async fn overview(
        &self,
    ) -> Result<BackupOverviewView, UiError>;

    async fn plan(
        &self,
        request: BackupPlanRequest,
    ) -> Result<BackupPlanView, UiError>;

    async fn start(
        &self,
        plan: BackupPlanId,
    ) -> Result<BackupJobView, UiError>;

    async fn cancel(
        &self,
        job: BackupJobId,
    ) -> Result<(), UiError>;

    async fn verify(
        &self,
        backup: BackupRef,
    ) -> Result<BackupVerificationView, UiError>;
}
```

---

# 173. Restore Presentation API

```rust
pub trait RestorePresentation {
    async fn inspect(
        &self,
        source: BackupSource,
    ) -> Result<BackupInspectionView, UiError>;

    async fn plan(
        &self,
        source: BackupSource,
        options: RestoreOptions,
    ) -> Result<RestorePlanView, UiError>;

    async fn start(
        &self,
        plan: RestorePlanId,
    ) -> Result<RestoreJobView, UiError>;

    async fn cancel(
        &self,
        job: RestoreJobId,
    ) -> Result<(), UiError>;
}
```

---

# 174. Export Presentation API

```rust
pub trait ExportPresentation {
    async fn plan(
        &self,
        request: ExportPlanRequest,
    ) -> Result<ExportPlanView, UiError>;

    async fn start(
        &self,
        plan: ExportPlanId,
        destination: ExportDestinationHandle,
    ) -> Result<ExportJobView, UiError>;
}
```

---

# 175. Import Presentation API

```rust
pub trait ImportPresentation {
    async fn inspect(
        &self,
        source: ImportSource,
    ) -> Result<ImportInspectionView, UiError>;

    async fn plan(
        &self,
        request: ImportPlanRequest,
    ) -> Result<ImportPlanView, UiError>;

    async fn start(
        &self,
        plan: ImportPlanId,
    ) -> Result<ImportJobView, UiError>;
}
```

---

# 176. Migration Presentation API

```rust
pub trait MigrationPresentation {
    async fn create_session(
        &self,
    ) -> Result<MigrationSessionView, UiError>;

    async fn inspect_session(
        &self,
        session: MigrationSessionId,
    ) -> Result<MigrationSessionView, UiError>;

    async fn start_transfer(
        &self,
        session: MigrationSessionId,
        scope: MigrationScope,
    ) -> Result<MigrationJobView, UiError>;

    async fn finalize(
        &self,
        session: MigrationSessionId,
        retire_old_device: bool,
    ) -> Result<MigrationResultView, UiError>;
}
```

---

# 177. Backup Events

```rust
pub enum BackupUiEvent {
    JobChanged(BackupJobView),
    BackupAdded(BackupHistoryItemView),
    BackupRemoved(BackupId),
    OverviewChanged(BackupOverviewView),
}
```

---

# 178. Restore Events

```rust
pub enum RestoreUiEvent {
    JobChanged(RestoreJobView),
    PlanInvalidated(RestorePlanId),
}
```

---

# 179. Android ViewModel

Owns:

```text
selected source/destination UI
SAF effects
confirmation screens
scope selection
progress presentation
secure-screen effects
```

Rust owns backup/restore truth.

---

# 180. Dioxus Presenter

Owns:

```text
file/folder dialog effects
history selection
progress view
confirmation dialogs
```

---

# 181. Platform Handle Rule

Android/desktop UI passes:

```text
destination/source handle
```

to Rust.

Do not load backup bytes into UI state.

---

# 182. Android SAF Source

Use:

```text
content URI / file descriptor
```

---

# 183. Desktop Source

Use:

```text
path/handle
```

validated by Rust.

---

# 184. No Raw Backup Bytes Over JNI

Hard rule.

---

# 185. Restore Key Entry

If backup requires key:

```text
sensitive input
show/hide
paste optional
```

---

# 186. Failed Key Entry

Show generic:

```text
Could not unlock this backup
```

Avoid cryptographic oracle details.

---

# 187. Retry Limits

Security policy may rate-limit repeated unlock attempts.

---

# 188. Backup Corruption Detail

Normal UI:

```text
Backup could not be verified
```

Advanced diagnostics may show non-secret structural reason.

---

# 189. Error Taxonomy

```rust
pub enum BackupUiErrorKind {
    DestinationUnavailable,
    PermissionDenied,
    StorageFull,
    SourceUnavailable,
    WrongRecoveryMaterial,
    CorruptBackup,
    UnsupportedVersion,
    MigrationFailed,
    VerificationFailed,
    ConflictRequiresAction,
    Cancelled,
    Internal,
}
```

---

# 190. Error UX

Storage full:

```text
Choose another destination
Manage Storage
```

Unsupported newer version:

```text
Update the app
```

Wrong recovery material:

```text
Could not unlock backup
```

---

# 191. Retry

Safe jobs can expose:

```text
Retry
```

---

# 192. Resume

If backend has checkpoints:

```text
Resume
```

---

# 193. Backup History Pagination

If many backups tracked, paginate.

---

# 194. Archive List

Separate from active backup history if necessary.

---

# 195. Accessibility — Backup

Screen reader:

```text
Last backup yesterday, verified
```

---

# 196. Progress Accessibility

Announce meaningful milestones, not every percent.

---

# 197. Restore Plan Accessibility

Conflict/warning summary must be readable.

---

# 198. Recovery Key Accessibility

Grouped reading.

---

# 199. Large Font

Scope lists/warnings wrap.

---

# 200. RTL

Backup screens mirror.

Filenames/technical IDs preserve appropriate direction.

---

# 201. Reduced Motion

Progress works without animated dependence.

---

# 202. Color Independence

Verified/failed/warning use text/icon.

---

# 203. Desktop Keyboard

Support:

```text
Tab
Enter
Esc
Ctrl/Cmd+F in history
```

---

# 204. Android TalkBack

SAF/open/save system UI remains platform accessible; app confirmation/progress must be equally accessible.

---

# 205. Screenshot Test Fixtures

Required:

```text
no backup
healthy backup
stale backup
failed backup
backup running
verifying
restore inspect
migration required
restore dry-run
restore conflict
restore running
export plaintext warning
migration transfer
complete
dark mode
large font
RTL
```

---

# 206. Backup Test Matrix

```text
small backup
large backup
cancel
power loss
storage full
provider unavailable
verification failure
restart
retention
last verified backup
```

---

# 207. Restore Test Matrix

```text
compatible
old version migration
newer unsupported
wrong account
wrong key
corrupt backup
selective restore
merge
cancel before commit
failure during commit
search rebuild
```

---

# 208. Migration Test Matrix

```text
new device
offline LAN transfer
interrupted transfer
resume
old device retained
old device revoked after success
failure before finalization
```

---

# 209. Export/Import Tests

```text
Markdown
JSON
media export
plaintext warning
unsupported import items
duplicate import
provenance
```

---

# 210. Security Tests

Ensure no restore can reactivate stale:

```text
ratchets
session keys
device auth tokens
hardware-bound secrets
```

---

# 211. Secret Leakage Tests

No:

```text
backup keys
recovery keys
plaintext export content
```

in logs/crash reports/analytics.

---

# 212. Large Data Test

Multi-gigabyte backup/restore/export remains streaming and memory-bounded.

---

# 213. Multi-Device Backup Semantics

Backup is a logical recovery artifact.

It need not mirror per-device:

```text
cache
download state
search index
```

---

# 214. Device-Local Data

May include optionally:

```text
drafts
device preferences
```

with clear policy.

---

# 215. Account-Wide Data

Typically:

```text
contacts
groups
messages
verification metadata
```

subject to backend architecture.

---

# 216. Backup Completeness Indicator

Can show:

```text
Includes messages and files
Excludes search index and temporary data
```

---

# 217. Restore Source Trust

A backup being decryptable does not automatically mean every imported external record is trustworthy.

---

# 218. Import vs Restore Trust

Restore from own authenticated backup can preserve more trust metadata than arbitrary import.

---

# 219. Archive Trust

Read-only archive preserves provenance but does not grant active device authorization.

---

# 220. Initial Production Scope

Ship:

```text
encrypted full backup
manual backup
verified backup history
restore inspect
restore dry run
safe full restore
new-device recovery
selective restore basics
export conversations/files
import supported portable format
device migration
Android SAF
desktop native file/folder dialogs
progress/cancel/retry
backup/recovery key integration
```

Defer:

```text
many cloud-provider-specific UIs
continuous cloud sync disguised as backup
complex retention DSL
cross-account restore
collaborative archives
```

unless backend explicitly requires them.

---

# 221. Definition of Done

UI/UX Part 16 is complete when:

- backup, restore, export, import, migration, and archive are distinct concepts
- Rust owns container format, encryption, integrity, version compatibility, migration, restore safety, and conflict policy
- backup status/history and verified recovery points are visible
- backup creation supports destination, scope, size estimate, progress, verification, cancellation, and safe finalization
- account recovery and backup recovery are not conflated unless intentionally unified
- restore starts with inspection and a non-mutating dry-run plan
- selective restore is possible without exposing internal storage schema
- stale sessions, ratchets, device tokens, and hardware-bound secrets are never blindly restored
- new-device restore creates fresh device security identity
- existing-device restore uses safe merge/replace semantics
- derived search indexes/thumbnails/caches rebuild instead of being trusted from backup
- migration differs from simply linking another device and never revokes the old device before success
- export clearly warns when output is plaintext/unprotected
- import preserves provenance and does not invent verification
- Android uses SAF/document-provider flows and no giant JNI byte arrays
- desktop uses native file/folder flows
- background/daemon execution and process recovery are defined
- accessibility, RTL, large font, progress semantics, and secure secret entry are explicit
- Rust backup/restore/export/import/migration presentation APIs are specified
- corruption, wrong key, incompatible version, storage failure, interruption, secret leakage, and large-data tests are included

---

# 222. Final Architecture

```text
                  RUST DATA SURVIVABILITY CORE
                            │
      ┌─────────────────────┼─────────────────────┐
      │                     │                     │
    Backup               Restore             Portability
      │                     │                     │
 Encrypt/Verify        Inspect/Plan        Export/Import
 Snapshot/Write        Migrate/Merge       Archive/Migrate
      │                     │                     │
      └─────────────────────┼─────────────────────┘
                            │
                  Backup Presentation
                    ┌───────┴───────┐
                    │               │
                 Dioxus          Compose
                    │               │
            Desktop Files      Android SAF
```

Restore safety:

```text
Backup
  │
  ▼
Inspect
  │
  ▼
Verify / Unlock
  │
  ▼
Compatibility
  │
  ▼
Dry-Run Restore Plan
  │
  ▼
Explicit Confirmation
  │
  ▼
Staged / Transactional Restore
  │
  ▼
Verification
  │
  ▼
Rebuild Derived State
```

---

# 223. Final Principle

Backup UX should optimize for a single outcome:

```text
when something goes wrong, the user can recover safely and understand what will happen before data is changed
```

The correct model is:

```text
encrypted verified backup
+
explicit recovery ownership
+
inspect-before-restore
+
dry-run planning
+
fresh device security
+
portable exports
```

not:

```text
copy application directories and hope they work later
```

This gives Dioxus desktop and Android Compose a recoverable, portable, security-conscious data lifecycle while the Rust core remains authoritative for backup correctness and restore safety.
