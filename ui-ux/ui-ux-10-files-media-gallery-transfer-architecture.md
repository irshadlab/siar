# UI/UX Part 10 — Files, Media Gallery & Transfer UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 10  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete files, shared-media, gallery, preview, upload/download, transfer-center, storage-management, platform file integration, and large-file UX across desktop and Android.

---

# 1. Purpose

Files are a first-class communication surface. Users will exchange:

```text
images
videos
documents
audio
voice notes
archives
code
PDFs
links
large binary files
```

The UX must remain understandable when a file is remote-only, partially downloaded, queued, paused, waiting for a peer, verifying, locally deleted, unavailable, or present only on another device.

> **The UI represents logical file and transfer state; Rust owns storage, encryption, integrity, availability, transfer scheduling, and authoritative lifecycle.**

---

# 2. Architectural Position

```text
Rust File / Blob Core
        │
        ├── Blob identity
        ├── Metadata
        ├── Encryption
        ├── Local availability
        ├── Remote availability
        ├── Transfer state
        ├── Integrity verification
        ├── Thumbnail state
        └── Storage policy
        │
        ▼
File Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Primary File UX Surfaces

```text
Files destination
Conversation shared media
Message attachment cards
Transfer center
Media viewer
File details
Storage management
```

---

# 4. Files Destination

Recommended categories:

```text
Recent
Images
Videos
Documents
Audio
Links
Transfers
```

Optional later:

```text
Favorites
Downloaded
Large Files
```

---

# 5. Stable File Identity

Use a logical identity such as:

```rust
pub struct BlobId([u8; 32]);
```

Never use a filename, local path, URI, or list index as authoritative file identity.

The same blob may appear in multiple messages or conversations while physical storage remains deduplicated.

---

# 6. File Summary DTO

```rust
pub struct FileSummaryView {
    pub blob: BlobId,
    pub source_message: Option<MessageId>,
    pub conversation: ConversationId,
    pub sender: AccountId,
    pub display_name: String,
    pub kind: FileKind,
    pub size: u64,
    pub created_at: Timestamp,
    pub availability: FileAvailability,
    pub transfer: Option<TransferSummaryView>,
    pub thumbnail: Option<ThumbnailRef>,
}
```

---

# 7. File Kinds

```rust
pub enum FileKind {
    Image,
    Video,
    Document,
    Audio,
    Archive,
    Code,
    Other,
    Link,
}
```

Use semantic kinds rather than relying only on MIME strings.

---

# 8. File Availability

```rust
pub enum FileAvailability {
    RemoteOnly,
    PartiallyAvailable,
    LocalVerified,
    LocalUnverified,
    Missing,
    Removed,
}
```

`LocalVerified` means the content exists locally and integrity verification succeeded.

`RemoteOnly` means metadata is known but the device does not currently possess the complete file.

`PartiallyAvailable` means resumable local transfer data exists.

---

# 9. Device-Local Availability

A fundamental rule:

```text
message/file metadata may sync account-wide
but local downloaded bytes are device-specific
```

If the phone downloads a 2 GB video, the desktop must not claim that it is locally available unless the desktop also has it.

---

# 10. Desktop Files Layout

Recommended:

```text
+---------------------------------------------------------------+
| Files     Search       Filter       Sort       View            |
+-----------------------+---------------------------------------+
| Categories            | Content                               |
| Recent                |                                       |
| Images                | Grid / list / details                 |
| Videos                |                                       |
| Documents             |                                       |
| Audio                 |                                       |
| Links                 |                                       |
| Transfers             |                                       |
+-----------------------+---------------------------------------+
```

Dioxus should support richer desktop density, resizing, keyboard navigation, multi-select, context menus, and optional preview/details pane.

---

# 11. Android Files Layout

Recommended:

```text
TopAppBar
    Files
    Search
    Sort / Filter

Category chips or secondary navigation

LazyGrid / LazyColumn
```

Use a grid for visual media and list presentation for documents/audio.

---

# 12. Tablet and Foldable

Expanded Android can use:

```text
NavigationRail / category pane
        │
File grid/list
        │
Optional preview/detail pane
```

Respect hinges and adaptive window size.

---

# 13. Recent Files

Sort by meaningful message/file activity, usually sent/received timestamp.

Do not reorder because a thumbnail finished generating or a local cache changed.

---

# 14. Image Gallery

Grid item:

```text
thumbnail
optional download-state overlay
optional multi-select state
```

Support date grouping such as:

```text
Today
Yesterday
This Month
Older
```

if useful.

---

# 15. Video Gallery

Grid item:

```text
thumbnail
duration
remote/download indicator
```

Do not decode full video for list presentation.

---

# 16. Documents

Prefer list/details presentation:

```text
filename
semantic type
size
conversation/sender
date
availability
```

---

# 17. Audio

List:

```text
name/type
duration if known
sender
date
availability
```

Voice notes may appear here while remaining primarily accessible through the timeline.

---

# 18. Links

Use Part 32 local indexing.

Display:

```text
title if available
domain
source conversation
date
```

Opening a link must pass through safe external URL handling.

---

# 19. Search

File search can include:

```text
filename
caption
conversation
sender
metadata
optional locally extracted document text
```

Search remains local-first.

---

# 20. Filtering

Potential filters:

```text
Conversation
Sender
Type
Date
Downloaded
Remote-only
```

Keep the normal UI simple; expose combinations through filter sheets/popovers.

---

# 21. Sorting

Recommended:

```text
Newest
Oldest
Name
Size
```

---

# 22. View Modes

Desktop:

```text
Grid
List
Details
```

Android:

```text
Grid for media
List for docs/audio/transfers
```

Persist view preference per device/category.

---

# 23. Shared Media per Conversation

Contact/group details should link to a scoped media area:

```text
Media
Files
Audio
Links
```

Each item retains its source `MessageId` when available.

Action:

```text
Show in conversation
```

jumps to the source message.

---

# 24. Source Message Missing

If the message was deleted but a retained local file still exists:

```text
Source message unavailable
```

The file view must not crash or navigate to a nonexistent row.

---

# 25. Thumbnail Architecture

Thumbnail generation is derived state.

```rust
pub enum ThumbnailState {
    None,
    Generating,
    Ready(ThumbnailRef),
    Failed,
}
```

The UI receives handles/references, never generates authoritative file state.

---

# 26. Thumbnail Rules

Use:

```text
stable placeholder immediately
known aspect ratio where available
small decoded thumbnail for grid/list
full content only in viewer
```

Thumbnail failure must not make the original file unusable.

---

# 27. Layout Stability

If image/video dimensions are known before thumbnail decode, reserve the expected aspect ratio to avoid large layout shifts.

---

# 28. Media Viewer

Opening supported media launches a dedicated viewer.

Desktop:

```text
main workspace
or detached media window
```

Android:

```text
full-screen destination
```

---

# 29. Viewer Actions

Common:

```text
Back/Close
Previous
Next
Save
Share
Forward
Info
Show in Conversation
```

Video adds:

```text
Play/Pause
Seek
```

---

# 30. Viewer Navigation

Previous/next operates within a logical collection:

```text
conversation images
recent videos
search result set
```

not the device filesystem directory.

---

# 31. Image Viewer

Support:

```text
zoom
pan
fit-to-screen
actual-size option on desktop
```

Android uses pinch-to-zoom.

Desktop may use mouse wheel/keyboard controls.

---

# 32. Video Playback

Use native/platform-appropriate media rendering.

Hard rule:

```text
raw decoded video frames do not flow through Compose/Dioxus state
```

UI receives renderer/player state and handles.

---

# 33. Audio Playback

UI consumes:

```text
playback state
position
duration
speed if applicable
```

The audio subsystem owns samples/buffers.

---

# 34. Document Preview

Initial recommendation:

```text
preview only formats that can be handled safely and cheaply
otherwise open externally
```

Do not build a complex office/PDF renderer merely for Part 10.

---

# 35. Text/Code Preview

Small text/code files may use a read-only viewer with:

```text
size cap
syntax highlighting optional
copy
open externally
```

---

# 36. Archives

Do not automatically extract untrusted archives.

Show:

```text
archive filename
size
type
open externally / save
```

---

# 37. Transfer Identity

```rust
pub struct TransferId([u8; 16]);
```

A `TransferId` is distinct from `BlobId`.

The same blob can participate in multiple transfer attempts over time.

---

# 38. Transfer Direction

```rust
pub enum TransferDirection {
    Upload,
    Download,
}
```

---

# 39. Transfer States

```rust
pub enum TransferState {
    Queued,
    WaitingForPeer,
    Connecting,
    Transferring,
    Paused,
    Verifying,
    Complete,
    Failed,
    Cancelled,
}
```

---

# 40. Transfer Summary

```rust
pub struct TransferSummaryView {
    pub id: TransferId,
    pub blob: BlobId,
    pub direction: TransferDirection,
    pub state: TransferState,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub rate: Option<TransferRate>,
    pub eta: Option<Duration>,
    pub failure: Option<TransferFailureView>,
    pub capabilities: TransferCapabilities,
}
```

---

# 41. Queued Is Not Failure

A transfer may be queued because of:

```text
offline state
scheduler priority
battery policy
metered policy
active call priority
peer unavailable temporarily
```

Use neutral language.

---

# 42. Waiting for Peer

User-facing examples:

```text
Waiting for sender
Waiting for recipient
Waiting for device
```

depending direction/context.

---

# 43. Transferring

Show when meaningful:

```text
percentage
completed / total bytes
speed
ETA when stable
```

Do not show unstable ETA values.

---

# 44. Progress Update Rate

Visible transfer progress should be coalesced to roughly:

```text
5–10 updates/sec maximum
```

Background/invisible transfers may update much less frequently.

---

# 45. Pause / Resume

Only show if Rust reports capability.

Do not infer support from UI state alone.

---

# 46. Cancel

Cancel means:

```text
stop this transfer
```

It does not automatically mean:

```text
delete message
delete file metadata
block peer
```

---

# 47. Retry

Retry a failed transfer without duplicating the logical file.

Use same `BlobId` and backend retry semantics.

---

# 48. Verification Phase

After all bytes arrive, show:

```text
Verifying…
```

A transfer becomes complete only after integrity verification succeeds.

---

# 49. Verification Failure

This is security-sensitive.

Show:

```text
File failed integrity verification
```

Do not allow normal Open action.

Potential actions:

```text
Retry
Delete partial data
Details
```

---

# 50. Failure Taxonomy

```rust
pub enum TransferFailureKind {
    PeerUnavailable,
    RouteUnavailable,
    StorageFull,
    SourceMissing,
    PermissionDenied,
    VerificationFailed,
    CancelledByPeer,
    Unsupported,
}
```

Presentation maps these to understandable language.

---

# 51. Transfer Center

Recommended sections:

```text
Active
Queued
Paused
Failed
Completed Recently
```

---

# 52. Desktop Transfer Center

Provide a dedicated destination plus an optional small utility panel showing active transfers.

Do not permanently occupy large screen space when nothing is happening.

---

# 53. Android Transfer Center

Files → Transfers screen.

Long user-visible transfers may also expose foreground-service notifications when justified by Android policy.

---

# 54. Transfer Row

```text
thumbnail/icon
filename
peer/conversation
state
progress
rate / ETA when useful
actions
```

---

# 55. Android Transfer Notification

Possible:

```text
File transfer in progress
42%
Pause / Cancel
```

Strict privacy mode can hide file/contact names.

---

# 56. Desktop Background Transfer

If the core/daemon remains active after window close:

```text
transfer continues
```

The tray may show a compact count.

---

# 57. Offline Transfers

If no usable route:

```text
Waiting for connection
```

Do not immediately mark failed.

---

# 58. Reconnection

When connectivity returns, Rust resumes/restarts according to transfer semantics.

UI keeps one logical row instead of creating a new visible transfer for every route attempt.

---

# 59. Direct / Relay / LAN Changes

Normal UI should not care.

A path switch does not change `TransferId` merely because the network path changes.

Advanced diagnostics may expose transport details.

---

# 60. Resumable Transfers

If supported, transfer state/checkpoints survive restart.

On UI restart:

```text
request fresh transfer snapshot
```

Never reconstruct byte progress from stale UI state.

---

# 61. Message Delivery vs Attachment Completion

These are distinct concepts.

```text
message delivered
≠
recipient has downloaded full attachment
```

The timeline can show the message while attachment transfer continues.

---

# 62. Upload Source Durability

Once an outbound attachment message is durably committed, Rust must have a source that is durable enough to satisfy the outbox contract.

UI should not assume the original Android URI or desktop path will remain forever.

---

# 63. Android Source Lifetime

Use:

```text
persisted URI permission
or
secure app staging
or
owned FD/stream lifecycle
```

according to provider capability.

---

# 64. Desktop Source Lifetime

For durable queued sends, stage/copy/reference according to Part 05/06 file subsystem policy.

If source disappears, show a semantic failure rather than an OS path exception.

---

# 65. Large Files

Hard UX requirements:

```text
no full-file UI memory buffering
non-blocking preparation
cancelability
progress when meaningful
storage preflight
```

---

# 66. Large File Preparation

Normal UI can show:

```text
Preparing…
```

Advanced details can show:

```text
Hashing
Staging
Encrypting metadata
```

if useful.

---

# 67. Memory Rule

Never transfer multi-gigabyte content through:

```text
Kotlin ByteArray
Compose state
Dioxus component Vec<u8>
```

Use handles, streams, descriptors, and Rust-owned buffers.

---

# 68. Auto-Download

User policy may include:

```text
Never
Wi-Fi only
Small files only
Always
```

Later per-type policy:

```text
Images
Video
Documents
Audio
```

---

# 69. Android Auto-Download Factors

Policy can incorporate:

```text
metered network
battery saver
storage pressure
background limits
```

---

# 70. Desktop Auto-Download

Desktop may use a more permissive default but should still obey storage/resource policy.

---

# 71. Unknown Request Attachments

Default:

```text
no automatic download
```

Possible UI:

```text
Attachment from unknown sender
Accept request to download
```

or explicit manual download if security policy allows.

---

# 72. Storage Architecture UX

Users should understand:

```text
app-managed internal copy
vs
exported external copy
```

The internal blob store remains authoritative for application availability.

---

# 73. Internal Storage First

Received files should normally reside in app-managed storage first.

Benefits:

```text
encryption
integrity
deduplication
cleanup
consistent references
```

Do not automatically scatter all received files into public Downloads.

---

# 74. Save / Export

User explicitly chooses:

```text
Save As
Save to device
Share outside app
```

The exported copy is separate from the internal app copy.

---

# 75. Delete Local Copy

Action wording:

```text
Remove from this device
```

This preserves message/file metadata if the content can be reacquired.

---

# 76. Delete Everywhere

Only offer if the message/file protocol explicitly supports it and user has permission.

Never combine with local cleanup.

---

# 77. Storage Management Screen

Recommended sections:

```text
Media
Documents
Audio
Partial Transfers
Thumbnail / Derived Cache
Per Conversation
Large Files
```

Search index storage can be shown separately as derived/rebuildable data.

---

# 78. Storage by Conversation

Example:

```text
School ERP Group       4.2 GB
Alice                   820 MB
Family                  510 MB
```

This helps users clean data intentionally.

---

# 79. Large Files Cleanup

List largest local blobs.

Potential actions:

```text
Remove from device
Show source conversation
Save externally first
```

---

# 80. Cache Cleanup

Safe derived data:

```text
thumbnails
temporary previews
```

Can be removed without losing authoritative user content.

---

# 81. Partial Transfer Cleanup

Show total space.

Cancel/remove partial state only with clear consequence:

```text
Download progress will be lost
```

if true.

---

# 82. Storage Pressure

If Rust reports low space:

```text
Storage is almost full
Manage Storage
```

The app should remain readable even if new files cannot be stored.

---

# 83. Save As — Desktop

Use native file dialog.

Flow:

```text
Choose destination
→ Rust prepares verified export stream/handle
→ write
→ success
```

Never expose internal CAS paths.

---

# 84. Open — Desktop

Depending type:

```text
built-in viewer
or
system default app
```

---

# 85. Reveal in Folder

Only for exported/user-visible filesystem copies.

Do not reveal private internal content-addressed storage.

---

# 86. Android Save

Use platform mechanisms such as:

```text
Storage Access Framework
MediaStore
CreateDocument
```

according to content type.

---

# 87. Android Open

Use a secure content URI/provider.

Do not expose raw app-private paths.

---

# 88. Android Share

Use the system share sheet with scoped temporary read permission.

---

# 89. Android Content Provider Security

Only expose explicitly authorized file handles/BlobIds.

Never create a generic provider that makes the entire internal blob store browsable.

---

# 90. URI Permission Lifetime

Grant only what the receiving app needs and revoke/expire appropriately.

---

# 91. Desktop Drag-In

Handled by Part 06 composer.

Dropped files become Rust-owned draft attachment sources.

---

# 92. Desktop Drag-Out

Future enhancement.

Could export a temporary file/handle to the OS drag system.

Not required for v1.

---

# 93. Share vs Forward

Use distinct terminology:

```text
Forward → another conversation inside the app
Share → another application through the OS
```

---

# 94. Forwarding

Rust owns re-encryption/reference semantics.

UI only chooses destination conversation(s).

---

# 95. Filenames

Support Unicode.

Sanitize export paths.

Prevent:

```text
../
absolute paths
reserved names
path separators used maliciously
```

---

# 96. Duplicate Filenames

Export/save should use collision-safe naming or ask user.

Logical identity remains `BlobId`.

---

# 97. File Type Safety

MIME is a hint, not sole authority.

Potentially dangerous files such as executables can show a warning before external open.

Do not claim malware detection unless actually implemented.

---

# 98. Missing File

Display:

```text
File unavailable
```

Possible reason/details:

```text
source no longer online
local copy removed
source missing
```

---

# 99. Personal / Archive Node

If a trusted archive source can supply a file later, UI may show:

```text
Download from archive
```

without exposing unnecessary storage topology.

---

# 100. Deduplication

Physical content deduplication is invisible.

If two messages reference identical bytes, each still appears in its own conversation context.

---

# 101. EXIF and Metadata

Do not surface sensitive photo location metadata by default.

Detailed metadata view can be added only if useful and privacy-safe.

---

# 102. Thumbnail Privacy

Thumbnails are sensitive derived data and follow app-lock/local-storage security policy.

---

# 103. Locked App

Files/gallery should not bypass app lock.

---

# 104. Sensitive Media

Optional setting:

```text
blur until opened
```

High-security Android mode may apply secure-window policy to selected viewer screens.

---

# 105. Search Privacy

File names, extracted text, and content stay local by default according to Part 32.

---

# 106. Accessibility — File Grid

Example screen-reader output:

```text
Photo from Alice, yesterday, downloaded
```

---

# 107. Accessibility — Document Row

```text
PDF, timetable.pdf, 2.4 megabytes, received from Alice, downloaded
```

---

# 108. Accessibility — Transfer

```text
timetable.pdf, downloading, 42 percent
```

Do not announce every percentage update automatically.

---

# 109. Media Viewer Accessibility

Expose labeled:

```text
Close
Previous
Next
Save
Share
Info
```

---

# 110. Large Font

Rows/cards must expand without clipping important actions or filename/state.

---

# 111. RTL

Layouts mirror correctly.

Technical identifiers, hashes, and canonical filenames may need controlled bidi handling.

---

# 112. Color Independence

Transfer state requires text/icon/progress semantics, not color alone.

---

# 113. Reduced Motion

Gallery and viewer transitions can be reduced/disabled.

---

# 114. Desktop Keyboard UX

Potential:

```text
Enter             Open
Space             Quick Preview
Ctrl/Cmd+S        Save As
Ctrl/Cmd+F        Search
Shift+F10         Context Menu
```

Deletion should never be a surprising single-key destructive action.

---

# 115. Desktop Multi-Select

Support for files:

```text
Ctrl/Cmd click
Shift click
```

Batch actions:

```text
Download
Save
Forward
Remove Local Copies
```

as capabilities allow.

---

# 116. Android Selection Mode

Long press enters contextual multi-selection.

Back exits selection before leaving screen.

---

# 117. Batch Download

Selected remote files can be queued.

Transfer center remains authoritative for progress.

---

# 118. Batch Cleanup

Show approximate freed space before confirmation.

---

# 119. Auto-Cleanup

Optional policy:

```text
Keep forever
30 days
90 days
custom
```

This is device-local storage policy unless explicitly synced.

---

# 120. Retention Integration

Part 33 may remove local blobs according to policy.

UI can show:

```text
Removed to save space
```

when metadata remains.

---

# 121. Backup Integration

Backup policy determines whether full file bytes, metadata only, recent files, or no files are included.

Part 10 only displays backup coverage where useful; it does not own backup truth.

---

# 122. Search Index Integration

If a local downloaded document was indexed and local content is later removed, Part 32 decides whether extracted content/vector state remains or is deleted.

---

# 123. File Lifecycle

Conceptually:

```text
message metadata known
        ↓
remote available
        ↓
partial local
        ↓
local verified
        ↓
optional external export
        ↓
local app copy may later be removed
```

The UI must represent this logical lifecycle, not merely `path.exists()`.

---

# 124. Rust File Presentation API

```rust
pub trait FilePresentation {
    async fn list(
        &self,
        query: FileListQuery,
    ) -> Result<FilePage, UiError>;

    async fn details(
        &self,
        blob: BlobId,
    ) -> Result<FileDetailView, UiError>;

    async fn download(
        &self,
        blob: BlobId,
    ) -> Result<TransferId, UiError>;

    async fn remove_local(
        &self,
        blob: BlobId,
    ) -> Result<(), UiError>;

    async fn source_message(
        &self,
        blob: BlobId,
    ) -> Result<Option<MessageId>, UiError>;
}
```

---

# 125. Transfer Presentation API

```rust
pub trait TransferPresentation {
    async fn list(
        &self,
        filter: TransferFilter,
    ) -> Result<Vec<TransferSummaryView>, UiError>;

    async fn pause(
        &self,
        transfer: TransferId,
    ) -> Result<(), UiError>;

    async fn resume(
        &self,
        transfer: TransferId,
    ) -> Result<(), UiError>;

    async fn cancel(
        &self,
        transfer: TransferId,
    ) -> Result<(), UiError>;

    async fn retry(
        &self,
        transfer: TransferId,
    ) -> Result<(), UiError>;
}
```

---

# 126. Transfer Capabilities

```rust
pub struct TransferCapabilities {
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub can_retry: bool,
}
```

The UI renders actions from these capabilities instead of hardcoded assumptions.

---

# 127. Export Presentation API

```rust
pub trait FileExportPresentation {
    async fn prepare_export(
        &self,
        blob: BlobId,
    ) -> Result<FileExportHandle, UiError>;
}
```

The platform UI then chooses destination/share mechanism.

---

# 128. Thumbnail Presentation API

```rust
pub trait ThumbnailPresentation {
    async fn thumbnail(
        &self,
        blob: BlobId,
        size: ThumbnailSize,
    ) -> Result<Option<ThumbnailRef>, UiError>;
}
```

---

# 129. File Events

```rust
pub enum FileUiEvent {
    FileUpdated(FileSummaryView),
    FileRemoved(BlobId),
    ThumbnailChanged {
        blob: BlobId,
        thumbnail: Option<ThumbnailRef>,
    },
    StoragePressureChanged(StoragePressureView),
}
```

---

# 130. Transfer Events

```rust
pub enum TransferUiEvent {
    TransferAdded(TransferSummaryView),
    TransferUpdated(TransferSummaryView),
    TransferRemoved(TransferId),
}
```

Only affected rows/items update.

---

# 131. Android ViewModel Ownership

Owns presentation state such as:

```text
selected category
filter/sort sheet
selection mode
viewer position
save/share one-off effects
```

Rust owns file/transfer truth.

---

# 132. Dioxus Presenter Ownership

Owns:

```text
view mode
selection
sidebar category
preview pane
platform dialog effects
```

Rust owns file/transfer truth.

---

# 133. No Raw File Bytes in UI DTOs

Hard rule.

UI receives:

```text
BlobId
ThumbnailRef
secure viewer/export handles
metadata
progress
```

not file contents.

---

# 134. Android Platform Effects

Examples:

```text
OpenCreateDocument
LaunchShareSheet
OpenExternalViewer
OpenMediaStoreExport
```

---

# 135. Desktop Platform Effects

Examples:

```text
OpenSaveDialog
OpenExternalApplication
RevealExportedFile
CreateMediaViewerWindow
```

---

# 136. Android Permission Philosophy

Prefer SAF/MediaStore/system picker/content URIs to broad storage permissions.

Request broad media access only if a real feature needs device-wide browsing.

---

# 137. Empty States

Examples:

```text
No shared files yet
No shared images
No active transfers
No downloaded files
```

Use one relevant action where possible.

---

# 138. Offline UX

Local verified files open normally.

Remote-only file shows:

```text
Waiting for connection
```

or a Download action that queues the request.

---

# 139. Loading UX

Local file metadata should render immediately.

Thumbnail generation and remote availability checks remain incremental.

---

# 140. Performance Requirements

The UI should support thousands of file records through:

```text
paging
virtualization
bounded thumbnail cache
incremental events
```

---

# 141. Image Grid Performance

Decode only visible and near-visible thumbnails.

---

# 142. Video Thumbnail Performance

Generate asynchronously and cache.

---

# 143. Large File Metadata

Do not synchronously inspect/hash huge content on the UI thread.

---

# 144. Stable Keys

Use:

```text
BlobId
TransferId
```

for grid/list identity.

---

# 145. Multi-Device Semantics

Example:

```text
Phone:
    Blob X = LocalVerified

Desktop:
    Blob X = RemoteOnly
```

Both know the same logical message/file metadata.

This is correct.

---

# 146. Transfer State Is Device-Local

Do not synchronize byte progress from one device into another device's transfer UI.

Each device may start its own download.

---

# 147. Shared Metadata

Can synchronize:

```text
BlobId
filename
size
type
message association
sender
```

subject to encryption/privacy model.

---

# 148. Notification Semantics

Do not notify for every tiny automatic thumbnail/image download.

Notify only for meaningful user-visible transfers or failures.

---

# 149. Screenshot Test States

Required fixtures:

```text
empty Files
image gallery
document list
remote-only media
downloading
paused
waiting for peer
verifying
failed verification
storage full
transfer center
media viewer
dark mode
large font
RTL
```

---

# 150. Android Test Matrix

Include:

```text
SAF save
MediaStore save
share URI
external viewer
process death
foreground transfer notification
metered network policy
storage full
foldable layout
TalkBack
```

---

# 151. Desktop Test Matrix

Include:

```text
grid/list/details
keyboard navigation
multi-select
Save As
external open
reveal exported file
detached viewer
daemon transfer continuity
```

---

# 152. Transfer Tests

```text
offline queue
pause/resume
peer disappears
route switch
verification
storage full
cancel
retry
restart
```

---

# 153. Security Tests

A verification failure must never transition to openable/complete UI state.

---

# 154. Path Safety Tests

Malicious filenames cannot escape export destinations or create unintended paths.

---

# 155. Large File Test

Multi-gigabyte files must not cause UI memory proportional to file size.

---

# 156. Thumbnail Failure Test

File remains accessible through metadata/open/download even when thumbnail generation fails.

---

# 157. Local Delete Test

Removing the local copy does not delete its message or peer copy unless user explicitly invokes a separate supported destructive action.

---

# 158. Initial Production Recommendation

Ship first:

```text
Recent files
Image gallery
Video gallery
Document list
Audio list
Links
Conversation shared media
Download
Transfer center
Pause/resume/cancel/retry when supported
Media viewer
Save / share / open
Remove local copy
Low-storage warning
```

Defer:

```text
cloud-like folders
collaborative editing
built-in office suite
advanced PDF editor
complex tags
favorites
remote media-streaming optimization
filesystem synchronization
```

---

# 159. Definition of Done

UI/UX Part 10 is complete when:

- files use stable logical `BlobId` identity rather than paths
- device-local availability is explicitly separated from synced logical metadata
- Recent/Images/Videos/Documents/Audio/Links/Transfers surfaces are defined
- conversation-scoped shared media is defined
- transfer states, actions, progress, and retry semantics are explicit
- queued/offline/waiting states are not treated as immediate errors
- Complete means integrity verification succeeded
- progress updates are throttled and capability-driven
- large files never travel through UI memory as giant buffers
- Android uses secure content URIs, FDs, SAF, and MediaStore-style flows
- desktop uses native Save/Open/Reveal semantics without exposing internal CAS paths
- media viewer semantics are defined
- thumbnails are derived, asynchronous, and non-authoritative
- removing a local copy is distinct from deleting the logical message/file everywhere
- storage management and low-storage UX are defined
- accessibility, keyboard/TalkBack, RTL, large font, and reduced motion are covered
- Rust file, transfer, export, thumbnail, and event presentation contracts are defined
- restart, offline, route switch, storage-full, verification-failure, malicious-filename, and multi-device cases are tested

---

# 160. Final Architecture

```text
                    RUST FILE / BLOB CORE
                             │
       ┌─────────────────────┼─────────────────────┐
       │                     │                     │
    Metadata             Local Storage         Transfers
       │                     │                     │
       └─────────────────────┼─────────────────────┘
                             │
                   File Presentation
                    ┌────────┴────────┐
                    │                 │
                 Dioxus            Compose
                    │                 │
            Desktop Files      Android Files
                    │                 │
                    └────────┬────────┘
                             │
                  BlobId / TransferId
```

Platform-specific output:

```text
Desktop:
    Save As
    Open
    Reveal
    detached viewer

Android:
    SAF / MediaStore
    secure content URI
    share sheet
    external viewer
```

The UI never owns:

```text
raw transfer chunks
encryption keys
internal CAS paths
full large-file buffers
```

---

# 161. Final Principle

The file UX should behave like a reliable communication attachment system, not an accidental filesystem browser.

The right model is:

```text
logical file identity
+
clear local/remote availability
+
resumable verified transfer state
+
platform-native open/save/share
+
bounded thumbnails/previews
+
device-aware storage policy
```

not:

```text
UI manipulates arbitrary paths and byte buffers directly
```

This gives Dioxus desktop and Android Compose a safe, fast, native file/media experience while the Rust file/blob and transfer engines remain authoritative.
