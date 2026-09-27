# UI/UX Part 06 — Message Composer, Attachments, Voice Notes & Drafts UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 06  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete message composition experience across desktop and Android, including text entry, drafts, replies, edits, attachments, paste/share flows, voice notes, camera/media capture, preview, reordering, offline queuing, validation, autosave, accessibility, process-death recovery, and Rust-backed send semantics.

---

# 1. Purpose

The composer is where user intent becomes durable outbound content.

It must handle:

```text
plain text
multiline text
reply
edit
attachments
images
video
documents
voice notes
paste
drag/drop
shared content
offline send
draft recovery
permission flows
```

without duplicating message-domain logic in the UI.

The governing principle is:

> **The composer owns temporary input interaction; Rust owns draft persistence, validation, attachment preparation, message creation, and send truth.**

---

# 2. Architectural Position

```text
User Input
   │
   ├── text
   ├── attachment
   ├── voice note
   ├── paste/drop/share
   └── reply/edit intent
   │
   ▼
Platform Composer UI
   │
   ▼
Composer Presenter / ViewModel
   │
   ▼
Rust Composition Service
   │
   ├── validation
   ├── draft persistence
   ├── attachment ingestion
   ├── message creation
   └── outbox
```

---

# 3. Composer State

Recommended shared presentation model:

```rust
pub struct ComposerSnapshot {
    pub conversation: ConversationId,
    pub mode: ComposerMode,
    pub text: String,
    pub attachments: Vec<ComposerAttachment>,
    pub can_send: bool,
    pub validation: ComposerValidation,
    pub draft_state: DraftState,
    pub voice_state: VoiceComposerState,
}
```

---

# 4. Composer Modes

```rust
pub enum ComposerMode {
    New,
    Reply {
        target: MessageReferenceView,
    },
    Edit {
        target: MessageId,
    },
}
```

Only one primary mode should be active at a time.

---

# 5. UI-Owned vs Rust-Owned Composer State

UI may own transient presentation details:

```text
text field focus
IME state
cursor position
attachment drag hover
expanded attachment sheet
recording gesture progress
```

Rust should own or validate:

```text
draft content if durable
reply target
edit target
attachment list
attachment identity
validation
send eligibility
```

---

# 6. Draft Strategy

Recommended:

```text
draft is a first-class Rust domain object
```

rather than only in ViewModel/component memory.

This improves:

```text
process-death recovery
desktop restart
device crash recovery
future multi-device draft sync
```

---

# 7. Draft Identity

```rust
pub struct DraftId([u8; 16]);
```

Usually one active draft per conversation.

---

# 8. Draft Model

```rust
pub struct MessageDraft {
    pub id: DraftId,
    pub conversation: ConversationId,
    pub mode: ComposerMode,
    pub text: String,
    pub attachments: Vec<DraftAttachmentRef>,
    pub updated_at: Timestamp,
}
```

---

# 9. Draft Is Not a Message

A draft must not:

```text
consume MessageSequence
trigger notifications
send typing receipts
appear as sent history
```

until user sends.

---

# 10. Autosave

Draft autosave should be:

```text
debounced
```

not persisted per keystroke.

Example:

```text
300–1000 ms after change
```

Tune by storage/backend.

---

# 11. Crash Safety

If application crashes after user typed:

```text
draft reappears
```

according to autosave boundary.

---

# 12. Draft Emptying

If user clears composer:

```text
delete empty draft
```

unless reply/edit state still meaningful.

---

# 13. Draft in Inbox

Part 04 may show:

```text
Draft: ...
```

from Rust draft projection.

---

# 14. Draft Sync

Initial recommendation:

```text
device-local
```

Later optional:

```text
account-wide encrypted draft sync
```

Do not require cross-device draft sync in v1.

---

# 15. Multi-Device Draft Conflict

If later synced:

```text
latest device edit
or
per-device drafts
```

must be explicit.

Avoid silent text merging.

---

# 16. Text Input

Support:

```text
Unicode
emoji
multiline
RTL
mixed scripts
clipboard
hardware keyboard
IME composition
```

---

# 17. Text Length

UI may show limit hint.

Rust remains authority.

---

# 18. Validation

Examples:

```text
message too long
too many attachments
attachment too large
unsupported file
editing not allowed
conversation read-only
```

---

# 19. Validation Timing

Use:

```text
lightweight local prevalidation
+
authoritative Rust validation
```

for responsive UX.

---

# 20. Send Button State

Enable when:

```text
text non-empty
or
valid attachment exists
or
valid voice note exists
```

and Rust says sending allowed.

---

# 21. Empty Whitespace

Trim policy must be defined.

Recommended:

```text
message consisting only of whitespace → invalid
```

while preserving meaningful leading/trailing spaces inside code blocks if supported.

---

# 22. Enter / Send Behavior

Android phone:

```text
Enter → newline
send button → send
```

Desktop:

```text
Enter → configurable
Ctrl/Cmd+Enter → send
```

Recommendation for desktop default:

```text
Enter → newline
Ctrl/Cmd+Enter → send
```

or user setting later.

---

# 23. IME Action

Android keyboard action can show:

```text
Send
```

only if product wants single-line-like behavior.

For multiline chats, explicit send button is clearer.

---

# 24. Composer Expansion

Text area expands vertically up to a bounded max height.

Then scroll internally.

---

# 25. Desktop Composer Height

Example:

```text
1–8 lines
```

before internal scrolling.

---

# 26. Android Composer Height

Example:

```text
1–6 lines
```

to preserve screen space above keyboard.

---

# 27. Reply Mode

When replying:

```text
reply context strip
```

appears above composer.

Contains:

```text
sender
message excerpt/type
cancel
```

---

# 28. Reply Context

Tap/click reply context can:

```text
jump to original
```

if useful.

---

# 29. Cancel Reply

Clears reply target but preserves typed text.

---

# 30. Edit Mode

Edit context must be visually distinct:

```text
Editing message
```

with:

```text
original text loaded
save action
cancel
```

---

# 31. Cancel Edit

Restores previous draft only if architecture explicitly preserved it.

Recommendation:

```text
editing temporarily replaces composer
```

and original draft is stashed locally/Rust-side until edit completes/cancels.

---

# 32. Edit Save

Rust validates:

```text
ownership
time/policy
message still exists
content constraints
```

---

# 33. Edit Conflict

If message changed remotely while editing:

```text
show updated state
offer reload/overwrite only if supported
```

Avoid silent overwrite.

---

# 34. Attachments Overview

Supported categories:

```text
Photos
Videos
Documents
Audio
Camera capture
Other files
```

---

# 35. Attachment Picker Architecture

UI requests platform picker.

Rust receives safe file handles/FDs/paths.

---

# 36. Android Attachment Flow

```text
Compose
→ Activity Result API
→ URI
→ content metadata
→ FD/stream handle
→ Rust file ingestion
```

---

# 37. Desktop Attachment Flow

```text
Dioxus
→ native file dialog / drag-drop
→ path/handle
→ Rust file ingestion
```

---

# 38. Never Copy Giant Files Through UI State

Hard rule:

```text
no multi-GB ByteArray in Kotlin
no giant Vec<u8> in Dioxus component state
```

Use:

```text
descriptor
path
stream
handle
```

---

# 39. Attachment Ingestion

Rust creates:

```rust
pub struct ComposerAttachment {
    pub id: ComposerAttachmentId,
    pub kind: AttachmentKind,
    pub display_name: String,
    pub size: u64,
    pub preview: Option<AttachmentPreviewRef>,
    pub state: ComposerAttachmentState,
}
```

---

# 40. Attachment States

```rust
pub enum ComposerAttachmentState {
    Inspecting,
    Ready,
    Preparing,
    Invalid,
    Failed,
}
```

---

# 41. Inspecting

Rust validates:

```text
type
size
readability
security policy
```

---

# 42. Ready

Can be sent.

---

# 43. Preparing

Examples:

```text
thumbnail
video metadata
image processing
encryption preparation
```

---

# 44. Invalid

Examples:

```text
too large
unsupported
not readable
```

---

# 45. Attachment Preview Strip

Display above composer.

Each item:

```text
thumbnail/icon
name
size/type
remove
state
```

---

# 46. Reordering Attachments

Useful if multi-attachment message preserves ordering.

Support drag on desktop.

Android can support long-press reorder later.

---

# 47. Initial Recommendation

Support attachment ordering in data model.

Manual reordering optional for v1.

---

# 48. Remove Attachment

Removes draft reference.

If Rust staged temporary file:

```text
release staging resource
```

when no longer referenced.

---

# 49. Attachment Count Limit

Rust enforces.

UI shows clear limit.

---

# 50. Duplicate Attachment

Policy:

```text
allow duplicates
or
coalesce identical selection
```

Recommendation:

```text
allow unless exact same handle selected accidentally in one picker batch
```

---

# 51. Clipboard Paste

Desktop:

```text
paste text
image
file path where available
```

Android:

```text
paste text
image/content URI if platform provides
```

---

# 52. Paste Image

Create attachment draft.

Do not encode large image into text field.

---

# 53. Drag and Drop

Desktop drop over conversation/composer:

```text
visual overlay
drop
ingest
preview
```

---

# 54. Drag Over State

Presentation-only.

---

# 55. Invalid Drop

Show:

```text
Unsupported file
File too large
```

inline/snackbar.

---

# 56. Share Into App

Android incoming `ACTION_SEND` / `ACTION_SEND_MULTIPLE`:

```text
external app
→ Kotlin intent
→ safe content handles
→ choose conversation
→ create composer draft
```

---

# 57. Share Text

Populate composer text.

---

# 58. Share Files

Populate attachment draft.

---

# 59. Share + Existing Draft

Conflict policy required.

Recommendation:

```text
ask:
Replace draft / Add to draft / Cancel
```

if existing draft non-empty.

---

# 60. Camera Capture

Android:

```text
camera permission
capture flow
result handle
Rust ingestion
```

---

# 61. Desktop Camera Capture

Optional.

Not required if desktop product initially uses file picker only.

---

# 62. Photo Capture

Use platform-native capture or camera module.

Avoid duplicating full camera stack in composer.

---

# 63. Video Capture

Likewise.

May need duration/file-size limit.

---

# 64. Image Preprocessing

Potential:

```text
orientation normalization
thumbnail generation
optional downscale
```

Rust-first where practical.

---

# 65. Original vs Compressed Image

User may choose:

```text
Original quality
Optimized
```

later.

---

# 66. Video Compression

Do not silently recompress large video in v1 unless product policy explicitly says.

---

# 67. Attachment Security

Rust must inspect:

```text
path safety
readability
size
type
```

before accepting.

---

# 68. Filename Display

Use safe display name.

Never trust filename as path.

---

# 69. MIME

Treat MIME as hint, not sole truth.

---

# 70. Voice Notes

Voice-note composition is distinct from live calls.

---

# 71. Voice Note States

```rust
pub enum VoiceComposerState {
    Idle,
    RequestingPermission,
    Recording,
    Paused,
    Preview,
    Preparing,
    Failed,
}
```

---

# 72. Voice Note Flow

```text
press/tap record
→ permission if needed
→ recording
→ stop
→ preview
→ send or discard
```

---

# 73. Android Microphone Permission

Requested contextually when recording begins.

---

# 74. Desktop Microphone Permission

Platform-specific if required.

---

# 75. Hold-to-Record vs Tap-to-Record

Android can support familiar hold-to-record interaction.

Desktop should prefer explicit click record/stop.

---

# 76. Android Hold-to-Record

Possible gestures:

```text
hold → record
slide left → cancel
slide up → lock recording
```

Only if interaction is thoroughly tested/accessibility-friendly.

---

# 77. Accessibility Alternative

Always provide explicit:

```text
Start recording
Stop
Cancel
```

actions.

Do not require gesture-only recording.

---

# 78. Recording Timer

Show duration.

---

# 79. Waveform

Optional visual aid.

Not correctness requirement.

---

# 80. Recording Buffer

Audio path should not store raw PCM in Compose state.

Rust/native audio subsystem owns media buffers.

---

# 81. Voice Recording Format

Use product-selected encoded format from Rust media pipeline.

UI only sees:

```text
duration
waveform summary
state
temporary clip ID
```

---

# 82. Pause Recording

Optional.

Useful for longer voice notes.

Not required for v1.

---

# 83. Voice Preview

After recording:

```text
play
scrub
delete
send
```

---

# 84. Voice Playback

Use media/audio service.

No giant audio data in UI state.

---

# 85. Voice Send

Rust turns temporary clip into attachment/message content.

---

# 86. Voice Draft Persistence

Policy options:

```text
persist unfinished voice note
or
discard on exit
```

Recommendation:

```text
persist completed preview clip
discard active in-progress raw recording on crash
```

unless robust crash-safe recording exists.

---

# 87. Interrupted Recording

If app backgrounds unexpectedly:

```text
pause/stop safely
```

according to platform policy.

---

# 88. Incoming Call During Voice Recording

Recommendation:

```text
pause/stop voice recording
preserve completed portion if possible
```

call gets priority.

---

# 89. Call While Composer Draft Exists

Draft remains intact.

---

# 90. Send While Offline

Rust accepts message into outbox if policy allows.

UI clears composer only after durable local message creation succeeds.

---

# 91. Important Send Rule

Do not clear input immediately on button tap before Rust accepts it.

Correct:

```text
tap Send
→ Rust creates durable pending message
→ success
→ clear draft/composer
```

---

# 92. Send Failure Before Durable Commit

Composer remains intact.

Show error.

---

# 93. Send Accepted but Network Offline

Composer clears.

Timeline shows:

```text
Queued
```

---

# 94. Double Send Protection

Disable duplicate send while same submission is being committed, or use `CommandId`.

---

# 95. Compose Recomposition Safety

Never invoke send from recomposition.

Only explicit event.

---

# 96. Dioxus Re-render Safety

Same.

---

# 97. Send Command

```rust
pub struct SendMessageCommand {
    pub command_id: CommandId,
    pub conversation: ConversationId,
    pub draft_id: Option<DraftId>,
    pub mode: SendMode,
    pub text: String,
    pub attachments: Vec<ComposerAttachmentId>,
}
```

---

# 98. Send Mode

```rust
pub enum SendMode {
    New,
    Reply(MessageId),
}
```

Edit is separate command.

---

# 99. Sending Attachment Message

Rust determines:

```text
message metadata
content keys
outbox
transfer state
```

UI does not manually coordinate message + file transfer.

---

# 100. Attachment Send Atomicity

If message references attachment:

```text
message + attachment intent
```

must be created consistently.

---

# 101. Preparation Before Send

Some attachments may need:

```text
metadata extraction
thumbnail
hash
```

before message can be committed.

---

# 102. Large File UX

If preparation takes time:

```text
Preparing…
```

in composer attachment card.

---

# 103. User Can Continue Typing

Attachment preparation should not freeze text input.

---

# 104. Send During Preparation

Two options:

```text
disable send until ready
```

or:

```text
allow send and finish preparation in outbox
```

Recommendation:

```text
allow only if backend can guarantee durable staged attachment
```

Otherwise disable until safely staged.

---

# 105. Attachment Auto-Upload

Do not upload before user sends unless product explicitly wants pre-upload.

For P2P architecture, staging locally before send is enough.

---

# 106. Draft Attachment Storage

Rust may copy/stage selected content into app-controlled secure storage if original URI/path may disappear.

---

# 107. Android URI Lifetime

Content URI permissions may not survive process death.

If draft persistence is desired:

```text
persist URI permission
or
copy safely into app-owned draft staging
```

depending provider/support.

---

# 108. Desktop Path Lifetime

Original path may move/delete.

For durable draft attachments:

```text
stage/copy or detect missing at send
```

---

# 109. Draft Staging Policy

Recommended:

```text
small/medium attachment → secure app staging
large attachment → durable source handle/reference when possible
```

with clear invalidation handling.

---

# 110. Missing Draft Attachment

On restore:

```text
File is no longer available
Remove / Re-select
```

---

# 111. Draft Storage Quota

Bound staged draft data.

---

# 112. Old Draft Cleanup

GC abandoned drafts after user-defined/system retention.

---

# 113. Draft Cleanup Safety

Do not delete staged data still referenced by active draft.

---

# 114. Conversation Read-Only

Examples:

```text
blocked
left group
archived read-only policy
security issue
```

Composer becomes disabled with clear reason.

---

# 115. Blocked Contact

Composer:

```text
You blocked this contact
Unblock
```

---

# 116. Removed from Group

Composer:

```text
You can no longer send messages to this group
```

---

# 117. Security Hold

If identity requires review:

```text
sending may be blocked
Review security
```

according to Part 28 policy.

---

# 118. Permissioned Group

If only admins can post:

```text
Only admins can send messages
```

---

# 119. Slow Mode

Future group feature.

Composer displays cooldown.

Not required initially.

---

# 120. Attachment Picker UX — Android

Recommended bottom sheet:

```text
Camera
Photos
Video
Document
Audio
Other File
```

---

# 121. Attachment Picker UX — Desktop

Toolbar/menu:

```text
Attach File
Attach Photo
```

plus drag/drop and paste.

---

# 122. Attachment Button

Should remain easy to reach without cluttering composer.

---

# 123. Composer Action Layout — Android

Recommended:

```text
[+] [ Text field................ ] [Voice/Send]
```

When text/attachments present:

```text
voice button → send
```

---

# 124. Composer Action Layout — Desktop

Recommended:

```text
Attachments / Emoji
Multiline Text Editor
Voice optional
Send
```

---

# 125. Emoji Picker

Android:

```text
system keyboard emoji is enough
```

Desktop may provide emoji picker later.

Not required in v1.

---

# 126. Sticker/GIF

Future extension.

Do not complicate base composer.

---

# 127. Mentions

Group composer may support:

```text
@name
```

---

# 128. Mention Suggestions

Triggered by:

```text
@
```

with group member search.

---

# 129. Mention Data

Rust supplies valid member IDs.

UI formats suggestions.

---

# 130. Mention Token

Do not encode mention only as plain display name.

Use logical ID in message content model.

---

# 131. Autocomplete

Potential:

```text
mentions
commands
emoji
```

Keep modular.

---

# 132. Slash Commands

Only if product needs.

Plugins may add safe commands later.

---

# 133. Plugin Composer Extensions

Part 24 may allow:

```text
attachment provider
composer action
structured message form
```

through safe declarative extension points.

---

# 134. Plugin Boundary

Plugin must not directly manipulate core composer internals.

Host exposes permissioned API.

---

# 135. Structured Messages

Future:

```text
location
poll
contact card
ERP record
```

Composer extension can produce typed payload.

---

# 136. Location Sharing

If added:

```text
explicit permission
preview
precision controls
```

not automatic.

---

# 137. Message Scheduling

Future feature.

Would require:

```text
scheduled outbox
```

and dedicated UX.

Not part of v1 composer.

---

# 138. Disappearing Message Timer

Future per-conversation setting.

Composer may show timer indicator.

Not necessary now.

---

# 139. Attachment Captions

Allow optional text caption in same message or per attachment depending product model.

---

# 140. Recommendation

For v1:

```text
one text body + ordered attachments
```

Simpler and interoperable.

---

# 141. Per-Attachment Caption

Can be added later.

---

# 142. File Names

User may rename attachment display name before send if product supports.

Not necessary initially.

---

# 143. Sensitive Attachment Warning

Potential:

```text
executable
archive
unknown file
```

warning.

Do not over-warn normal documents.

---

# 144. Metadata Stripping

Optional privacy feature for photos:

```text
remove EXIF location
```

before send.

---

# 145. Default Recommendation

Strip sensitive location metadata from images by default if technically safe, or offer setting.

Rust media/file preparation owns it.

---

# 146. Original Metadata Option

Advanced user can preserve original if desired.

---

# 147. Clipboard Privacy

Do not automatically read clipboard.

Only read on explicit paste.

---

# 148. Android Clipboard Toast

Respect platform behavior.

---

# 149. Voice Note Privacy

Do not upload recording during capture unless user sends.

---

# 150. Draft Privacy

Drafts are sensitive local data.

Encrypt/protect under same local storage security policy.

---

# 151. Draft Notification

Do not show draft text in OS notifications.

---

# 152. Draft Search

Drafts may appear in inbox preview.

Do not include in global message search unless product explicitly wants.

---

# 153. Typing Indicator Trigger

Typing service can observe composer activity.

---

# 154. Typing Start

When user meaningfully begins input.

---

# 155. Typing Stop

On:

```text
send
clear
focus loss
idle
conversation leave
```

---

# 156. Attachment-Only Typing

Do not send "typing" merely because attachment picker is open.

---

# 157. Voice Recording Indicator

Part 30 may send recording indicator while active.

---

# 158. Privacy Setting

If user disables typing/recording indicators:

```text
composer still works
```

without sending ephemeral hints.

---

# 159. Focus Behavior — Desktop

Open conversation:

```text
focus timeline or composer based on navigation intent
```

New-conversation command:

```text
focus composer
```

---

# 160. Focus Behavior — Android

Opening conversation should not automatically open keyboard unless user entered via compose intent.

---

# 161. Restore Focus

After closing attachment sheet:

```text
return to composer
```

if appropriate.

---

# 162. Sending Focus

After send:

```text
keep composer focused
```

for rapid desktop chatting.

Android may keep keyboard open.

---

# 163. Accessibility — Composer

Text field label:

```text
Message
```

Reply context announced.

Attachment controls have meaningful labels.

---

# 164. Voice Note Accessibility

Explicit controls:

```text
Start recording
Stop recording
Play recording
Delete recording
Send recording
```

---

# 165. Attachment Preview Accessibility

Example:

```text
PDF attachment timetable.pdf, 2.4 MB, remove
```

---

# 166. Send Button Accessibility

Announce disabled reason if useful:

```text
Send unavailable, attachment still preparing
```

---

# 167. Large Font

Composer must expand without covering send/attachment controls.

---

# 168. RTL

Text field respects input direction.

Attachment controls and reply strips mirror correctly.

---

# 169. Reduced Motion

Attachment add/remove and voice recording animations should not depend on motion.

---

# 170. Color Independence

Recording state should use:

```text
icon
label
timer
```

not red color alone.

---

# 171. Desktop Keyboard Shortcuts

Potential:

```text
Ctrl/Cmd+Enter → Send
Esc → Cancel reply/edit
Ctrl/Cmd+Shift+A → Attach file
```

---

# 172. Android Hardware Keyboard

Support:

```text
Ctrl+Enter → Send
Esc/Back → dismiss context
```

where feasible.

---

# 173. Desktop Paste Files

OS clipboard file list can be converted into attachment draft.

---

# 174. Desktop Screenshot Paste

Image clipboard becomes attachment.

---

# 175. Android Image Paste

Support where IME/content APIs provide rich content.

---

# 176. Rich Content from IME

Android keyboards may insert:

```text
GIF
image
```

through content APIs.

Map to attachment ingestion if supported.

---

# 177. Validation UI

Prefer inline near affected item.

Examples:

```text
File exceeds 2 GB limit
Message is too long
```

---

# 178. Global Composer Error

Use above composer for:

```text
Conversation no longer writable
```

---

# 179. Snackbar

Good for:

```text
Attachment removed
Draft restored
```

only if useful.

---

# 180. No Modal for Ordinary Validation

Do not interrupt with dialog for simple file-size error.

---

# 181. Send In Progress

Short local commit should be near-instant.

Do not show blocking spinner unless unusual.

---

# 182. Large Attachment Preparation

Show progress on attachment tile.

---

# 183. Attachment Hashing

If backend hashes large file:

```text
Preparing 42%
```

optional.

---

# 184. User Cancel Preparation

Allow remove/cancel.

---

# 185. Composer Backgrounding

Android:

```text
save draft
release UI-only state
```

---

# 186. Process Death

On restart:

```text
restore draft from Rust
revalidate attachment staging
```

---

# 187. Desktop App Restart

Same.

---

# 188. Conversation Switch

Before leaving:

```text
flush/debounce draft save
```

---

# 189. Rapid Conversation Switching

Drafts remain per conversation.

---

# 190. Multiple Windows Desktop

Two windows open same conversation.

Potential conflict.

Recommendation:

```text
shared Rust draft
```

with one live draft state.

---

# 191. Draft Edit Conflict Across Windows

If both edit simultaneously:

```text
last-writer or explicit lock
```

Recommendation:

```text
single active draft lease per conversation per device
```

if this becomes real issue.

---

# 192. Draft Lease

Could expose:

```text
Draft active in another window
```

for multi-window safety.

---

# 193. Attachment Staging Across Windows

Owned by Rust draft ID, not window.

---

# 194. Composer Search/Slash UI Layer

Autocomplete popup should not block typing.

---

# 195. Mention Popup — Desktop

Anchored below text cursor if feasible.

---

# 196. Mention Popup — Android

Dropdown/popup above IME/composer.

---

# 197. Mention Selection

Inserts structured mention token.

---

# 198. Message Formatting Toolbar

Desktop may optionally expose:

```text
bold
italic
code
```

later.

Android can rely on markdown syntax or contextual toolbar.

---

# 199. Formatting Simplicity

Recommendation v1:

```text
basic plain text
links
emoji
code formatting only if important
```

Avoid heavy rich-text editor complexity initially.

---

# 200. Code Message UX

If supporting code blocks:

```text
monospace
preserve whitespace
```

Rust validation must not trim meaningful code whitespace.

---

# 201. Draft Version

```rust
pub struct DraftRevision(pub u64);
```

Useful for stale update prevention.

---

# 202. Composer Event

```rust
pub enum ComposerEvent {
    DraftChanged(ComposerSnapshot),
    AttachmentUpdated(ComposerAttachment),
    VoiceStateChanged(VoiceComposerState),
    WritePermissionChanged(bool),
}
```

---

# 203. Composer Commands

```rust
pub enum ComposerCommand {
    SetText(String),
    SetReply(Option<MessageId>),
    BeginEdit(MessageId),
    CancelEdit,
    AddAttachment(AttachmentSource),
    RemoveAttachment(ComposerAttachmentId),
    StartVoiceRecording,
    StopVoiceRecording,
    DiscardVoiceRecording,
    Send,
}
```

---

# 204. Avoid Per-Keystroke JNI Chatter

Android text updates can be debounced/batched to Rust draft persistence.

ViewModel may hold immediate local text while periodically syncing draft.

---

# 205. Immediate UI Text

Compose must remain responsive even if JNI/storage is briefly busy.

---

# 206. Draft Sync Strategy Android

Recommended:

```text
Compose TextField immediate state
→ ViewModel
→ debounce
→ Rust draft update
```

---

# 207. Draft Sync Strategy Desktop

Dioxus signal immediate.

Debounced Rust draft persistence.

---

# 208. Send Uses Latest Text

Before send:

```text
flush latest local text to Rust/send command payload
```

so debounce cannot lose final characters.

---

# 209. Rust Validation on Send

Rust checks:

```text
current conversation writable
text
attachments
reply/edit target
security
resource limits
```

---

# 210. Send Result

```rust
pub enum SendCommitResult {
    Committed(MessageView),
    Rejected(UiError),
}
```

---

# 211. Composer Clear

Only after:

```text
Committed
```

---

# 212. Draft Deletion

Committed send deletes/advances corresponding draft atomically.

---

# 213. Reply Clear

Reply target cleared after successful commit.

---

# 214. Attachment Ownership Transfer

On send:

```text
draft attachment
→ message/outbox attachment
```

atomically.

---

# 215. Voice Clip Ownership Transfer

Same.

---

# 216. Edit Commit

Separate:

```rust
pub struct EditMessageCommand {
    pub message: MessageId,
    pub new_text: String,
}
```

Attachments on edits may be disallowed initially for simplicity.

---

# 217. Recommended v1 Edit Scope

Allow:

```text
text edit only
```

Do not allow changing attachments initially.

---

# 218. Reply + Attachments

Allowed.

---

# 219. Attachment-Only Message

Allowed.

---

# 220. Empty Caption

Valid if attachment exists.

---

# 221. Voice-Only Message

Allowed.

---

# 222. Voice + Text

Possible:

```text
voice attachment + caption
```

if message model supports.

Recommendation:

```text
support later
```

unless already easy.

---

# 223. Sending Multiple Attachments

One message with ordered list is recommended.

Avoid creating N separate messages automatically unless user chooses.

---

# 224. Album Layout

Timeline can render image groups as album.

Part 05 renderer consumes attachment grouping.

---

# 225. File Attachment Group

Documents can render stacked.

---

# 226. Camera Capture Failure

Return to composer intact.

---

# 227. Permission Denied

Composer shows contextual explanation.

---

# 228. Storage Full

Cannot stage attachment/draft.

Show:

```text
Storage is full
Manage Storage
```

---

# 229. Read-Only External URI

If file can be read once but not persist:

```text
copy into staging
```

before draft can survive process death.

---

# 230. Temporary Camera File

Rust/file subsystem takes ownership after capture.

---

# 231. Draft Backup

Part 33 may include drafts optionally.

Recommendation:

```text
user drafts included in full device backup
```

but not search index.

---

# 232. Draft Export

Not necessary.

---

# 233. Draft Encryption

Use local app encryption/security policy.

---

# 234. Draft Telemetry

Never send draft text to analytics.

---

# 235. Composer Logging

Do not log:

```text
draft text
attachment filenames if sensitive
voice content
```

---

# 236. Crash Reports

Redact text field state.

---

# 237. Performance

Typing latency must remain local and immediate.

Do not block UI on:

```text
disk
network
hashing
thumbnail generation
```

---

# 238. Attachment Preparation Worker

Background Rust worker.

---

# 239. Voice Recording Priority

Realtime audio capture higher priority than:

```text
backup
semantic indexing
```

---

# 240. Draft Autosave Priority

Low but prompt.

---

# 241. Composer Memory

Avoid holding full attachments in memory.

---

# 242. Thumbnail Memory

Bound/cache.

---

# 243. Very Large Paste

If user pastes huge text:

```text
validate length
offer file conversion only if product supports
```

---

# 244. Long Text

Display character count near limit.

---

# 245. Mention Count Limit

Bound mentions if protocol needs.

---

# 246. Attachment Abuse

Unknown conversation/request restrictions may disallow attachment sends until accepted.

---

# 247. Message Request Composer

If user replying to unknown request:

```text
Accept / Reply
```

policy must be explicit.

---

# 248. Blocked Conversation

Composer disabled.

---

# 249. Archived Conversation

Archive should usually remain writable when opened.

Sending may automatically unarchive.

---

# 250. Unarchive on Send

Recommended:

```text
sending in archived conversation → unarchive
```

---

# 251. Group Role Change

If permission changes while typing:

```text
composer disables
draft preserved
```

---

# 252. Security State Change

If contact identity changes:

```text
send may pause
draft preserved
Review Security
```

---

# 253. Connection Loss During Composition

No visual disruption beyond subtle offline status.

---

# 254. Connection Return

No composer reset.

---

# 255. Duplicate Send Race

Use `CommandId`.

---

# 256. Slow JNI Call

UI send button may briefly show commit state.

But avoid spinner if commit fast.

---

# 257. Android Back Behavior

Back while attachment sheet open:

```text
close sheet
```

Back while voice preview:

```text
stay conversation, optionally discard confirmation if unsaved
```

Back while composer has draft:

```text
navigate back, draft autosaved
```

---

# 258. Desktop Esc Behavior

Priority:

```text
close popup
cancel selection
cancel reply/edit
then normal navigation
```

Do not clear ordinary draft on Esc.

---

# 259. Attachment Preview Fullscreen

Tap attachment draft preview:

```text
open local preview
```

before send.

---

# 260. Remove from Preview

Available.

---

# 261. Image Crop/Edit

Future.

Not required in v1.

---

# 262. Video Trim

Future.

Not required.

---

# 263. Voice Noise Processing

Part 26 DSP may apply recording profile if desired.

---

# 264. Voice Recording Quality

Product setting:

```text
voice optimized
```

not user codec complexity.

---

# 265. Audio Focus

Voice recording must integrate with:

```text
media playback
calls
notifications
```

through Android/platform audio layer.

---

# 266. Recording While Music Playing

Policy:

```text
request audio focus
duck/pause other app according to Android behavior
```

---

# 267. Desktop Recording Device

Use current/default mic.

Advanced input selection in Calls & Media settings.

---

# 268. Recorder Failure

Examples:

```text
mic unavailable
permission denied
device disconnected
```

Show inline.

---

# 269. Attachment MIME Preview Mapping

UI gets semantic:

```text
Image
Video
PDF
Archive
Document
Audio
Unknown
```

not raw MIME string only.

---

# 270. International Filenames

Support Unicode safely.

---

# 271. Truncation

Truncate display name visually.

Full filename in tooltip/details.

---

# 272. Attachment Sorting

Preserve user-selected order.

---

# 273. Draft Timestamp

Used for inbox preview and cleanup.

---

# 274. Draft Restore Banner

Usually unnecessary.

Simply restore content.

Optional subtle:

```text
Draft restored
```

after crash if helpful.

---

# 275. Draft Corruption

If draft cannot load:

```text
show recovery message
```

without affecting conversation history.

---

# 276. Stale Reply Target

If reply target deleted:

```text
reply context says Original unavailable
```

user may still send reply or cancel.

---

# 277. Stale Edit Target

If edit target deleted:

```text
exit edit mode
preserve typed replacement as draft if possible
```

---

# 278. Attachment Revalidation on Restore

Check:

```text
staged file exists
permissions still valid
size/type
```

---

# 279. Sendability Projection

Rust can expose:

```rust
pub enum ComposerSendability {
    Ready,
    Empty,
    Preparing,
    ReadOnly,
    Invalid,
    SecurityBlocked,
}
```

---

# 280. UI Presentation

Map to:

```text
enabled send button
disabled reason
banner
inline warning
```

---

# 281. Android ViewModel

Owns:

```text
immediate text
focus
picker effects
permission effects
temporary reorder UI
```

Rust owns durable draft and attachment preparation.

---

# 282. Dioxus Presenter

Owns:

```text
input signal
focus
drag hover
popup state
```

Rust owns durable draft.

---

# 283. Shared Presentation API

```rust
pub trait ComposerPresentation {
    async fn snapshot(
        &self,
        conversation: ConversationId,
    ) -> Result<ComposerSnapshot, UiError>;

    async fn save_draft(
        &self,
        update: DraftUpdate,
    ) -> Result<DraftRevision, UiError>;

    async fn add_attachment(
        &self,
        source: AttachmentSource,
    ) -> Result<ComposerAttachment, UiError>;

    async fn remove_attachment(
        &self,
        id: ComposerAttachmentId,
    ) -> Result<(), UiError>;

    async fn send(
        &self,
        command: SendMessageCommand,
    ) -> Result<SendCommitResult, UiError>;
}
```

---

# 284. Voice API

```rust
pub trait VoiceComposerService {
    async fn start(&self, conversation: ConversationId) -> Result<(), UiError>;
    async fn stop(&self) -> Result<VoiceDraft, UiError>;
    async fn discard(&self) -> Result<(), UiError>;
}
```

Platform adapter handles permission/audio device integration.

---

# 285. Draft Update

```rust
pub struct DraftUpdate {
    pub draft: DraftId,
    pub expected_revision: DraftRevision,
    pub text: String,
    pub mode: ComposerMode,
}
```

---

# 286. Revision Protection

Prevents stale delayed autosave overwriting newer draft.

---

# 287. Android Debounce Race

Example:

```text
type A
type B
send
old autosave A arrives later
```

Revision/flush prevents old draft resurrection.

---

# 288. Desktop Multi-Window Race

Same protection.

---

# 289. Composer Test Matrix

Required states:

```text
empty
text
multiline
reply
edit
attachment
multiple attachments
preparing
invalid attachment
voice recording
voice preview
offline
read-only
security blocked
restored draft
```

---

# 290. Android Tests

Verify:

```text
IME open/close
permission request
file picker
share intent
process death
rotation
large font
TalkBack
RTL
```

---

# 291. Desktop Tests

Verify:

```text
drag/drop
paste image
keyboard send
Esc behavior
multi-window draft consistency
file dialog
```

---

# 292. Draft Tests

Scenario:

```text
type text
kill app
restart
```

Expected:

```text
draft restored
```

---

# 293. Send Commit Test

If Rust commit fails:

```text
draft remains
```

---

# 294. Offline Send Test

If commit succeeds offline:

```text
draft clears
timeline shows Queued
```

---

# 295. Attachment Restore Test

Draft references missing file.

UI shows reselect/remove.

---

# 296. Voice Interrupt Test

Incoming call interrupts voice recording safely.

---

# 297. Permission Denied Test

Mic/camera denied without losing draft.

---

# 298. Draft Revision Test

Stale autosave cannot overwrite newer draft.

---

# 299. Large File Test

No giant memory copy.

Preparation stays responsive.

---

# 300. Multiple Attachment Test

Order preserved.

Remove one without affecting others.

---

# 301. Reply/Delete Race

Target deleted while composing reply.

No crash.

---

# 302. Edit/Delete Race

Target deleted while editing.

Edit exits safely.

---

# 303. Security Change Test

Identity warning appears.

Draft preserved while sending blocked.

---

# 304. Accessibility Test

Entire composer usable with:

```text
keyboard
TalkBack/screen reader
large text
no gestures
```

---

# 305. Performance Targets

Typing response:

```text
immediate local
```

Draft autosave:

```text
background
```

Attachment ingestion:

```text
non-blocking
```

Voice controls:

```text
low-latency
```

---

# 306. Initial Production Recommendation

For v1, support:

```text
plain/multiline text
reply
text edit
durable drafts
file/photo/video/document attachments
drag/drop desktop
system picker Android
paste image/file where supported
voice notes
offline send
attachment preview/remove
```

Defer:

```text
rich editor
GIF/sticker marketplace
image editor
video trim
scheduled messages
complex slash commands
per-attachment captions
multi-device draft sync
```

---

# 307. Definition of Done

UI/UX Part 06 is complete when:

- composer temporary interaction state is separated from Rust durable draft/message truth
- drafts survive app/process restart according to autosave policy
- stale autosave cannot overwrite newer draft
- reply and edit modes are explicit and recover safely
- composer clears only after Rust durably commits a pending message
- offline send produces queued timeline state rather than losing input
- Android uses system picker/URI/FD flows rather than giant byte arrays
- desktop supports file dialog, drag/drop, and paste
- attachments are represented by Rust-owned draft attachment IDs
- attachment preparation/validation is asynchronous and non-blocking
- missing draft attachments are detected on restore
- voice-note recording never puts raw PCM in Compose/Dioxus state
- voice recording has accessible non-gesture controls
- permissions are contextual and platform-native
- process death, call interruption, permission denial, security block, and offline transitions preserve draft safely
- attachment count/size/type limits are authoritative in Rust
- accessibility, RTL, large font, reduced motion, keyboard, and screen-reader behavior are defined
- send, attachment, draft, and voice operations have explicit Rust presentation APIs
- v1 scope remains focused and avoids premature rich-editor complexity

---

# 308. Final Architecture

```text
                    PLATFORM COMPOSER
             ┌──────────────┼──────────────┐
             │              │              │
           Text        Attachments       Voice
             │              │              │
             └──────────────┼──────────────┘
                            │
                    Presenter/ViewModel
                            │
                    Rust Composer Service
              ┌─────────────┼─────────────┐
              │             │             │
           Drafts       Attachment     Voice Draft
              │          Staging           │
              └─────────────┼─────────────┘
                            │
                      Send Commit
                            │
                         Outbox
                            │
                    Message Timeline
```

Platform-specific ingress:

```text
Desktop:
    file dialog
    drag/drop
    clipboard
    microphone

Android:
    system picker
    share intent
    camera
    microphone
    IME
```

All converge into the same:

```text
DraftId
ComposerAttachmentId
SendMessageCommand
MessageId
```

---

# 309. Final Principle

The composer should feel immediate and forgiving while preserving strong message-delivery correctness.

The right model is:

```text
instant local typing
+
durable debounced drafts
+
platform-native file/media input
+
Rust-owned validation/staging
+
atomic send commit
+
offline outbox
```

not:

```text
UI directly assembles and transmits message payloads
```

This makes composition fast and native on both Dioxus desktop and Android Compose while keeping the shared Rust communication engine authoritative.
