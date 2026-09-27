# UI/UX Part 26 — Performance, Virtualization & Large-Data UI Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 26  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete performance, virtualization, large-data rendering, paging, memory-bounding, incremental-update, media-loading, event-coalescing, multi-window, backpressure, instrumentation, and stress-test architecture required for production-scale messaging and local-first data.

---

# 1. Purpose

The platform must remain responsive with:

```text
millions of messages
hundreds of thousands of files
thousands of conversations
large contact lists
large group memberships
long-running transfer histories
large search result sets
many plugins
multiple windows
active calls
background sync
```

A UI that works only with a small test database is not production-ready.

The governing principle is:

> **The UI should render only what the user can currently perceive or imminently needs, while Rust owns bounded paging, incremental state, backpressure, and expensive data preparation.**

---

# 2. Architectural Position

```text
Rust Domain / Storage
       │
       ├── indexed queries
       ├── cursor paging
       ├── bounded projections
       ├── incremental events
       ├── thumbnail jobs
       ├── search paging
       ├── transfer progress
       └── backpressure
       │
       ▼
Presentation Service
       │
  Bounded Snapshots
  Incremental Events
       │
   ┌───┴────────┐
   │            │
Dioxus       Compose
Desktop      Android
```

---

# 3. Performance Objectives

Primary objectives:

```text
fast first useful paint
stable scrolling
bounded memory
minimal re-render/recomposition
responsive input
no network/database storms
predictable background work
graceful degradation under load
```

---

# 4. Core Rule

Never load:

```text
entire message history
entire file catalog
entire search corpus
entire contact DB
```

into UI memory.

---

# 5. Stable Identity

All large lists require stable identifiers.

Examples:

```text
ConversationId
MessageId
BlobId
AccountId
TransferId
PluginId
SecurityEventId
```

---

# 6. Never Use List Index as Identity

Hard rule.

---

# 7. Pagination Model

Prefer:

```text
cursor-based paging
```

over:

```text
offset paging
```

for mutable timelines.

---

# 8. Cursor Types

Example:

```rust
pub struct MessageCursor {
    pub sequence: MessageSequence,
}
```

---

# 9. Page Size

Tune by workload.

Typical initial ranges:

```text
messages: 50–100
conversations: 50–200
files: 50–200
search: 25–100
contacts: 100+
```

Exact values benchmarked.

---

# 10. Overscan

Virtualized lists render:

```text
visible items
+
small overscan
```

not entire page if framework allows.

---

# 11. Windowed Data

UI holds a bounded moving window around viewport.

---

# 12. Timeline Window

Example:

```text
previous page
current visible region
next page
```

with eviction outside bound.

---

# 13. History Eviction

Old pages can be evicted from UI memory while retained in Rust/storage.

---

# 14. Scroll Anchor

Eviction must not change logical position.

Use:

```text
stable item ID + offset
```

---

# 15. Message Timeline Architecture

```text
Recent Page
   │
   ▼
Virtualized Timeline
   │
   ├── prepend older page
   ├── append new messages
   └── evict distant pages
```

---

# 16. Initial Conversation Open

Load:

```text
recent 50–100 messages
```

from local storage.

---

# 17. Older History

Fetch when approaching top.

---

# 18. Newer History

Usually live events append.

For deep search jump:

```text
load around MessageId
```

---

# 19. Around-Target Paging

Important for search.

API:

```rust
page_around(message_id)
```

---

# 20. Scroll-to-Message

Use stable target.

Do not binary-search UI list.

---

# 21. Prepend Preservation

When older page inserted:

```text
preserve first visible MessageId + visual offset
```

---

# 22. New Messages While Scrolled Up

Do not force scroll to bottom.

---

# 23. Bottom Affinity

If user is at bottom:

```text
auto-follow
```

within threshold.

---

# 24. New Message Chip

When not at bottom:

```text
N new messages
```

---

# 25. Message Height Variability

Messages can contain:

```text
text
images
files
voice
replies
reactions
system events
```

So virtualization must support variable height.

---

# 26. Height Estimation

Can cache measured heights keyed by:

```text
MessageId + layout width class + text scale
```

---

# 27. Height Cache Invalidation

Invalidate on:

```text
edit
reaction layout change
attachment metadata change
width change
text scale change
```

---

# 28. Avoid Full Timeline Relayout

Only affected rows should update.

---

# 29. Message Event Granularity

Events:

```text
Inserted
Updated
Removed/Tombstoned
ReactionChanged
DeliveryChanged
```

---

# 30. Do Not Replace Whole Conversation Snapshot for Every Receipt

Hard rule.

---

# 31. Conversation List Performance

Potential thousands of rows.

Use:

```text
cursor paging
virtualization
summary DTO
coalesced updates
```

---

# 32. Conversation Summary Must Be Lightweight

Do not include:

```text
full latest message object
full participant list
full avatar bytes
```

---

# 33. Presence Updates

Presence changes should not reorder list.

---

# 34. Typing Updates

Update only affected row.

---

# 35. Unread Updates

Update row + badge.

---

# 36. Conversation Reorder

Only durable recency activity should reorder.

---

# 37. Reorder Animation

Use sparingly.

Burst updates should coalesce.

---

# 38. File Gallery Performance

Potential large catalog.

Use:

```text
paged metadata
virtualized grid
lazy thumbnails
```

---

# 39. Thumbnail Policy

Load only:

```text
visible
near-visible
```

items.

---

# 40. Thumbnail Sizes

Generate bounded variants:

```text
small
medium
viewer preview
```

not full original for grid.

---

# 41. Original Media

Never decode full-resolution image just to show thumbnail.

---

# 42. Image Decode

Rust/native media pipeline performs decode.

UI receives:

```text
image handle
texture handle
URI
platform image reference
```

depending platform.

---

# 43. Avoid Byte Copies

Especially:

```text
Android JNI
```

No giant:

```text
ByteArray
Vec<u8>
```

crossing presentation layer.

---

# 44. File Grid Reflow

Changing column count should not reload metadata.

---

# 45. File Scroll Anchor

Preserve:

```text
BlobId
```

nearest viewport.

---

# 46. Search Performance

Search results are paged.

---

# 47. Search Request Cancellation

Every query gets:

```text
SearchRequestId
```

---

# 48. Stale Query Result

Ignored if request no longer current.

---

# 49. Search Debounce

Typical:

```text
100–300 ms
```

depending platform.

---

# 50. Search UI Never Waits for Entire Corpus

First page returns quickly.

---

# 51. Search Provider Aggregation

If plugins contribute:

```text
core results
plugin results
```

can arrive independently.

---

# 52. Partial Provider Failure

Does not block other results.

---

# 53. Contacts Performance

Large contacts list:

```text
indexed local query
paged list
lazy presence
```

---

# 54. Presence Subscription Scope

Subscribe only for:

```text
visible contacts
active conversation
small important set
```

---

# 55. Group Member Performance

Large group lists must be paged.

---

# 56. Member Presence

Lazy.

---

# 57. Group Receipt Performance

Do not render per-recipient receipt inline for large group.

Use:

```text
aggregate
+
details page on demand
```

---

# 58. Transfer Center Performance

Many transfers require:

```text
throttled progress
virtualized list
```

---

# 59. Progress Frequency

Visible transfer progress:

```text
~5–10 updates/second maximum
```

often less.

---

# 60. Background Transfer Progress

Can update at much lower rate.

---

# 61. Progress Coalescing

If transport produces hundreds of events/sec:

```text
coalesce to latest visible value
```

---

# 62. Call Metrics

Developer metrics can update:

```text
1–2 Hz
```

normally.

Do not render per packet.

---

# 63. Presence Event Coalescing

Burst presence changes should collapse to latest state per account.

---

# 64. Typing Keepalives

Do not render on every keepalive.

Only state transitions/TTL.

---

# 65. Notification Badge Updates

Coalesce.

---

# 66. Global Event Bus

UI should not receive every low-level runtime event.

---

# 67. Presentation Event Layer

Rust converts low-level events into semantic UI events.

---

# 68. Bounded Channels

All async UI event channels bounded.

---

# 69. Backpressure

If UI falls behind:

```text
coalesce replaceable state
drop low-value diagnostics events
preserve durable/high-priority events
```

---

# 70. Event Priority

Recommended:

```rust
pub enum UiEventPriority {
    Critical,
    High,
    Normal,
    Low,
    Replaceable,
}
```

---

# 71. Critical Events

Examples:

```text
security state
SOS
call end
device revoked
```

must not be dropped.

---

# 72. Replaceable Events

Examples:

```text
transfer progress
typing state
presence
performance metrics
```

can coalesce.

---

# 73. Snapshot + Events Pattern

Recommended:

```text
Initial bounded snapshot
+
incremental semantic events
```

---

# 74. Resynchronization

If event stream gap detected:

```text
request fresh snapshot
```

---

# 75. Sequence Numbers

Presentation stream can use monotonic:

```text
UiRevision
```

---

# 76. Event Gap

UI detects:

```text
expected revision 101
received 104
```

then resnapshot.

---

# 77. Do Not Depend on Perfect Event Delivery

Hard rule.

---

# 78. UI Cache Layers

Possible:

```text
view-model cache
image cache
height cache
page cache
```

all bounded.

---

# 79. Bounded Memory

Every cache needs:

```text
max entries
max bytes
eviction policy
```

---

# 80. LRU

Useful for:

```text
thumbnails
message heights
recent pages
```

---

# 81. Pinning

Do not evict currently visible/active page.

---

# 82. Memory Pressure

On memory pressure:

```text
drop previews
drop distant pages
drop decoded thumbnails
```

before authoritative state.

---

# 83. Android Memory Pressure

Respond to lifecycle/system memory callbacks.

---

# 84. Desktop Memory Pressure

Use internal thresholds and OS signals where available.

---

# 85. Low-Memory Mode

Can reduce:

```text
overscan
thumbnail cache
page cache
animation
prefetch
```

---

# 86. Data Remains in Rust/Storage

UI cache eviction is not data deletion.

---

# 87. Compose Performance Architecture

Key rules:

```text
stable immutable UI models
stable keys
small StateFlow scopes
derivedStateOf where useful
avoid giant screen state objects
```

---

# 88. Compose Recomposition Scope

Do not have one global:

```text
AppState
```

causing whole app recomposition.

---

# 89. Feature ViewModels

Separate:

```text
InboxViewModel
ConversationViewModel
CallViewModel
FilesViewModel
SearchViewModel
```

---

# 90. StateFlow Granularity

Expose stable screen-level state.

High-frequency progress can use narrower flow.

---

# 91. LazyColumn

Use:

```text
key = MessageId
contentType = message kind
```

where supported.

---

# 92. LazyGrid

Use stable:

```text
BlobId
```

keys.

---

# 93. Compose Item Stability

Avoid recreating unrelated row models on every event.

---

# 94. Compose Remember

Use only for presentation-local state.

---

# 95. No Domain Truth in rememberSaveable

Hard rule.

---

# 96. Compose Derived Data

Heavy sorting/filtering belongs in Rust.

---

# 97. Compose Main Thread

Never perform:

```text
database query
hashing
large image decode
file I/O
crypto
```

on main thread.

---

# 98. Android JNI Boundary

Use coarse calls.

---

# 99. JNI Callback Flood

Bound/coalesce before crossing.

---

# 100. JNI Handles

For files/media use:

```text
FD
URI
opaque handle
```

not bytes.

---

# 101. Dioxus Performance Architecture

Key rules:

```text
small signals
stable keyed lists
local subscriptions
memoized derived views
virtualized large collections
```

---

# 102. Dioxus Re-render Scope

Avoid root-level signal changes for:

```text
typing
transfer progress
presence
```

---

# 103. Component Locality

Row-specific state updates row-specific signal/store.

---

# 104. Desktop Virtualization

Use virtual list/grid implementation capable of:

```text
variable height
stable keys
scroll anchor
```

---

# 105. Dioxus Main Thread

Do not block UI with Rust work even though language is shared.

Use async/background workers.

---

# 106. Shared Rust Does Not Mean Shared Thread

Hard rule.

---

# 107. Worker Pools

Separate:

```text
I/O async
CPU-heavy blocking
media processing
search indexing
```

---

# 108. Tokio Runtime

UI-facing async tasks should remain bounded.

---

# 109. spawn_blocking

Use for blocking CPU/FS work where appropriate.

---

# 110. Rayon

Potential for CPU parallelism in:

```text
thumbnail generation
indexing
media metadata
```

but resource-bounded.

---

# 111. Priority Scheduler

Background work priority:

```text
Emergency
Calls
Security
Messages
Visible UI
Transfers
Search indexing
Backup
Maintenance
```

---

# 112. UI Visibility Signal

Presentation layer can tell core:

```text
which screen/items are visible
```

to prioritize work.

---

# 113. Visible Set Model

Example:

```rust
pub struct VisibleRange<T> {
    pub first: T,
    pub last: T,
}
```

---

# 114. Use Cases

```text
read receipts
thumbnail priority
presence subscription
prefetch
```

---

# 115. Visibility Is Advisory

Rust validates semantics.

---

# 116. Prefetch

Prefetch only likely next content.

Examples:

```text
next message page
nearby thumbnails
adjacent file preview
```

---

# 117. Avoid Aggressive Prefetch on Mobile

Respect:

```text
battery
metered network
memory
```

---

# 118. Network-Aware Prefetch

Disable/reduce on:

```text
mobile data
battery saver
offline
```

---

# 119. Desktop Prefetch

Can be more aggressive within memory budgets.

---

# 120. Startup Performance

Goal:

```text
render shell
load local inbox
be interactive
```

before:

```text
network sync
index rebuild
plugin startup
```

completes.

---

# 121. Startup Staging

Recommended:

```text
Stage 1 UI shell
Stage 2 local DB
Stage 3 recent projections
Stage 4 network/runtime
Stage 5 background services
Stage 6 plugins/indexing
```

---

# 122. Plugin Startup

Do not block core app readiness.

---

# 123. Search Index Startup

Background.

---

# 124. Thumbnail Warmup

Background/visible-only.

---

# 125. Daemon Connection Desktop

UI can show cached/local state while reconnecting if architecture permits.

---

# 126. Cold Start Budget

Set measurable budget per platform.

Example targets should be benchmark-driven.

---

# 127. First Useful Paint

More important than:

```text
all services fully initialized
```

---

# 128. Navigation Performance

Switching between already-loaded main destinations should feel immediate.

---

# 129. Navigation Data Loading

Use cached bounded snapshot then refresh.

---

# 130. Conversation Open

Local recent page should be near-instant.

---

# 131. Search First Page

Target low latency from local index.

---

# 132. File Grid

Show metadata placeholders before thumbnails.

---

# 133. Image Pipeline

```text
metadata
→ thumbnail request
→ cached thumbnail
→ decode
→ display
```

---

# 134. Thumbnail Deduplication

Same BlobId/size request only once.

---

# 135. Cancellation

If item scrolls far away:

```text
cancel/deprioritize decode
```

---

# 136. Video Thumbnail

Generate asynchronously.

---

# 137. Audio Waveform

Derived data.

Load only when needed.

---

# 138. Avatar Pipeline

Bounded cache.

---

# 139. Avatar Dedup

Stable AccountId.

---

# 140. Animated Media

Avoid auto-playing many animations in lists.

---

# 141. GIF/Animated Image Policy

If supported:

```text
static preview until visible/focused
```

---

# 142. Video Autoplay

Avoid by default.

---

# 143. Rich Link Preview

Lazy fetch/generate.

Do not block message render.

---

# 144. Markdown Rendering

If messages support rich formatting:

```text
parse/cache asynchronously
```

bounded.

---

# 145. Syntax Highlighting

Developer/code messages:

```text
lazy
bounded
```

---

# 146. Search Snippet Generation

Rust/search engine generates snippet.

UI does not scan full message.

---

# 147. Large Message

Bound display.

Potential:

```text
Show More
```

for extremely long text.

---

# 148. Full Message Access

User can expand.

---

# 149. Pathological Content

Protect against:

```text
million-character message
huge Unicode grapheme sequences
deep nested markup
```

with protocol/UI bounds.

---

# 150. Emoji/Unicode Performance

Use grapheme-aware processing without O(n²) behavior.

---

# 151. Text Measurement Cache

Useful for variable-height timelines.

---

# 152. Width-Keyed Text Layout

Cache invalidates on width/text scale/font changes.

---

# 153. Multi-Window Performance

Multiple windows should share:

```text
core data
image cache where possible
transport
```

---

# 154. Window-Local UI Cache

Each window can have separate:

```text
scroll/page window
focus
selection
```

---

# 155. Avoid Duplicate Heavy Work

Two windows viewing same image should reuse decoded/thumbnail result if safe.

---

# 156. Call Window Priority

Active call UI gets high rendering priority.

---

# 157. Background Window

Can reduce:

```text
animations
metrics refresh
thumbnail decode
```

---

# 158. App Background Android

Stop unnecessary UI subscriptions.

---

# 159. Lifecycle Subscription

ViewModel/presentation subscribes only while needed.

---

# 160. Process Background

Core runtime may continue messaging/calls according to platform policy.

UI rendering stops.

---

# 161. Event Subscription Ownership

Every subscription has clear lifecycle.

---

# 162. Leak Prevention

On screen/window close:

```text
unsubscribe
cancel UI-only jobs
release handles
```

---

# 163. Resource Handle Lifetime

File/media handles scoped.

---

# 164. Image Handle Eviction

Release GPU/native image resources when cache evicts.

---

# 165. GPU Memory

Large media viewer must avoid keeping many full-res images decoded.

---

# 166. Viewer Neighbor Prefetch

Maybe:

```text
current
previous
next
```

only.

---

# 167. Ultra-High-Resolution Images

Use tiled decode if necessary.

---

# 168. Zoomed Image

Decode appropriate region/level if supported.

---

# 169. Video Playback

Media pipeline separate from UI state.

---

# 170. Audio PCM

Never stored in normal UI model.

---

# 171. Call Video Frames

Direct renderer/surface path.

---

# 172. Surface Rebinding

Orientation/layout change rebinds surface without restarting media.

---

# 173. Performance Budget Categories

Define budgets for:

```text
startup
navigation
scroll frame time
search first page
message send feedback
thumbnail decode
memory
event backlog
```

---

# 174. Example Budget Model

```rust
pub struct UiPerformanceBudget {
    pub startup_first_paint_ms: u32,
    pub navigation_p95_ms: u32,
    pub input_latency_p95_ms: u32,
    pub max_event_backlog: u32,
}
```

Actual numbers determined by benchmark hardware.

---

# 175. Reference Hardware

Test at least:

```text
low/mid Android
high-end Android
8 GB desktop
modern desktop
```

---

# 176. Slow Hardware

UI should degrade gracefully.

---

# 177. Performance Modes

Possible:

```text
Normal
Battery Saver
Low Memory
Emergency
```

---

# 178. Performance Mode Effects

Can reduce:

```text
animation
prefetch
thumbnail quality
background indexing
plugin work
```

---

# 179. Telemetry

Performance telemetry should be privacy-safe.

---

# 180. Safe Metrics

Potential:

```text
frame time
startup latency
query latency
event backlog
cache hit rate
memory class
```

without message content/identity.

---

# 181. Local Performance Diagnostics

Part 20 can expose:

```text
frame p95
memory
queue depth
search latency
```

---

# 182. Profiling Builds

Developer mode/builds can expose more detail.

---

# 183. Production Overhead

Instrumentation must be low-overhead.

---

# 184. Trace Sampling

Do not trace every message event indefinitely.

---

# 185. Performance Regression Gates

CI should fail on significant regressions in benchmark suite.

---

# 186. Benchmark Scenarios

Required:

```text
10k conversations
1M messages
100k files
100k search results
10k contacts
10k group members
100 active transfers history
multiple plugins
```

Synthetic fixtures.

---

# 187. Timeline Stress

Test:

```text
rapid incoming messages
edits
reactions
receipts
typing
scrolling
```

simultaneously.

---

# 188. Inbox Stress

Test frequent row updates without full-list rerender.

---

# 189. Search Stress

Rapid queries:

```text
a
al
ali
alic
alice
```

with cancellation.

---

# 190. File Grid Stress

Fast scroll through thousands of media items.

---

# 191. Thumbnail Stress

Cache misses + decode storm.

Verify bounded concurrency.

---

# 192. Transfer Stress

Hundreds of progress sources coalesced.

---

# 193. Presence Stress

Large presence burst.

---

# 194. Plugin Event Stress

Malicious/noisy plugin cannot flood UI.

---

# 195. Multi-Window Stress

Main + call + diagnostics + media window.

---

# 196. Memory Stress

Run long session.

Check:

```text
page eviction
image cache eviction
no unbounded signal/state growth
```

---

# 197. Leak Tests

Repeatedly:

```text
open/close conversation
open/close viewer
start/end call
open/close plugin panel
```

---

# 198. Process Restart Test

Caches are rebuilt lazily.

---

# 199. Database Growth Test

Performance should scale with:

```text
visible query size
```

not entire DB size where indexes exist.

---

# 200. Index Requirements

Large-data UI requires storage indexes for:

```text
conversation recency
message sequence
search
file type/date
contact name
group member lookup
```

---

# 201. UI Cannot Compensate for Bad Query Design

Hard rule.

---

# 202. N+1 Query Prevention

Presentation queries should return required summary data in batches.

---

# 203. Avatar N+1

Avoid separate DB query per row.

---

# 204. Presence N+1

Use batch subscription/query.

---

# 205. Attachment Metadata N+1

Batch/load with page projection.

---

# 206. Search Result Source Metadata

Batch.

---

# 207. DTO Size Budget

Presentation DTOs should be small.

---

# 208. Message Summary vs Full Message

Use separate models if needed.

---

# 209. File Summary vs File Detail

Separate.

---

# 210. Contact Summary vs Profile

Separate.

---

# 211. Progressive Detail Loading

List loads summary.

Detail loads full data.

---

# 212. Avoid Huge Nested DTOs

Hard rule.

---

# 213. Serialization

Desktop in-process may use direct Rust types.

Daemon/JNI boundaries use compact typed serialization/FFI.

---

# 214. Postcard

Suitable for internal binary transport where already chosen.

---

# 215. JSON

Only external/debug interop where necessary.

---

# 216. Copy Minimization

Use:

```text
Arc
handles
references
zero-copy buffers where safe
```

inside Rust.

---

# 217. UI Ownership

Do not expose lifetimes/borrowing complexity to Compose/Dioxus APIs.

---

# 218. Pagination API

```rust
pub trait PagedPresentation<Item, Cursor> {
    async fn page(
        &self,
        cursor: Option<Cursor>,
        direction: PageDirection,
        limit: u32,
    ) -> Result<Page<Item, Cursor>, UiError>;
}
```

---

# 219. Page Model

```rust
pub struct Page<T, C> {
    pub items: Vec<T>,
    pub previous: Option<C>,
    pub next: Option<C>,
    pub revision: UiRevision,
}
```

---

# 220. Incremental Event Model

```rust
pub enum CollectionUiEvent<T, Id> {
    Inserted {
        item: T,
    },
    Updated {
        id: Id,
        item: T,
    },
    Removed {
        id: Id,
    },
    ResetRequired {
        revision: UiRevision,
    },
}
```

---

# 221. High-Frequency State

Use separate replaceable channel.

Example:

```rust
pub struct TransferProgressEvent {
    pub id: TransferId,
    pub progress: f32,
}
```

---

# 222. Visibility API

```rust
pub trait VisibilityPresentation {
    async fn report_visible_messages(
        &self,
        conversation: ConversationId,
        range: VisibleRange<MessageId>,
    ) -> Result<(), UiError>;

    async fn report_visible_files(
        &self,
        range: VisibleRange<BlobId>,
    ) -> Result<(), UiError>;
}
```

---

# 223. Visibility Report Rate

Throttle.

Do not send per pixel scroll.

---

# 224. Read Receipt Observation

Use viewport observations at bounded cadence.

---

# 225. Performance Event

```rust
pub enum PerformanceUiEvent {
    MemoryPressure(ResourcePressure),
    DataWindowReset(UiRevision),
    BackgroundWorkThrottled,
}
```

---

# 226. Compose JNI Page Transfer

Send page metadata + compact DTOs.

No full DB rows/raw media.

---

# 227. Dioxus In-Process Page Transfer

Use shared Rust DTOs.

---

# 228. Daemon Mode Desktop

Use local typed IPC with:

```text
snapshot paging
incremental events
revisioning
```

same semantics.

---

# 229. Backpressure Across IPC

Bound queues.

---

# 230. Reconnect to Daemon

Resnapshot visible screens.

---

# 231. Accessibility and Virtualization

Part 21 applies.

---

# 232. Screen Reader Tree

Only bounded visible/nearby nodes.

---

# 233. Focus Preservation

Stable IDs.

---

# 234. Pagination Announcement

Only announce meaningful:

```text
Older messages loaded
```

if needed.

---

# 235. Large Text

May increase item heights and reduce viewport count.

Virtualization remains correct.

---

# 236. RTL

Does not change stable identity/paging.

---

# 237. Reduced Motion

Can reduce layout animation cost.

---

# 238. Error/Loading States

Part 24 integrates with paging.

---

# 239. Page Load Failure

Keep current page visible.

Show:

```text
Couldn't load older messages
Retry
```

---

# 240. Partial Paging

Do not blank list on one page failure.

---

# 241. Search Page Failure

Keep existing results.

---

# 242. Thumbnail Failure

Show placeholder.

Do not fail file list.

---

# 243. Media Decode Failure

Affects item/viewer only.

---

# 244. Low Storage

Pause non-essential caches/prefetch.

---

# 245. Background Priority

Indexing and backup yield to visible UI.

---

# 246. Emergency Priority

Emergency mode can disable almost all speculative work.

---

# 247. Thermal Pressure

Reduce media/background work.

---

# 248. Battery Saver

Reduce:

```text
prefetch
presence refresh
thumbnail concurrency
semantic indexing
```

---

# 249. Performance Settings

Normal users should not need performance tuning.

---

# 250. Advanced Settings

Maybe:

```text
Reduce Data Usage
Reduce Motion
Clear Cache
```

not cache sizes/thread counts.

---

# 251. Developer Diagnostics

Can expose:

```text
cache hit/miss
page count
event rate
queue depth
recomposition/re-render rate
```

---

# 252. No User-Facing FPS Obsession

Unless developer mode.

---

# 253. Failure Containment

One slow subsystem cannot freeze app.

Examples:

```text
plugin
search
thumbnail
backup
```

---

# 254. Timeout Boundaries

Plugin/extensions and external integrations must have timeouts.

---

# 255. Cancellation Tokens

User navigation cancels/deprioritizes stale UI-only work.

---

# 256. Search Cancellation

Mandatory.

---

# 257. Thumbnail Cancellation

Recommended.

---

# 258. Detail Load Cancellation

If user leaves screen.

---

# 259. Durable Job Cancellation

Different: backup/transfer semantics owned by core.

---

# 260. UI Cancellation vs Domain Cancellation

Do not confuse:

```text
stop observing progress
```

with:

```text
cancel transfer
```

---

# 261. Performance Regression Test Matrix

Required:

```text
large inbox
million-message conversation history
rapid live traffic
large file grid
rapid search typing
multi-window
low memory
battery saver
plugin flood
slow disk
slow CPU
```

---

# 262. Android Performance Tests

Verify:

```text
Compose recomposition scope
LazyColumn stability
LazyGrid stability
JNI event rate
process background/foreground
memory trim
```

---

# 263. Desktop Performance Tests

Verify:

```text
Dioxus rerender scope
virtual list
multi-window
daemon IPC
window resize
large diagnostics log
```

---

# 264. Scroll Performance

Measure:

```text
p50/p95 frame time
dropped frames
```

on reference hardware.

---

# 265. Input Latency

Measure:

```text
typing
send button
search input
navigation
```

---

# 266. Startup Benchmark

Cold/warm.

---

# 267. Memory Benchmark

Long-running.

---

# 268. Search Benchmark

First page and next page.

---

# 269. Thumbnail Benchmark

Decode queue saturation.

---

# 270. Event Backlog Benchmark

Simulate high-rate events.

---

# 271. Leak Benchmark

Repeated navigation loops.

---

# 272. Release Gate

Block release on major regression in:

```text
core scroll
startup
input latency
memory growth
```

---

# 273. Initial Production Scope

Ship:

```text
cursor paging
virtualized conversation/message/file/contact/search lists
stable IDs
message scroll-anchor preservation
bounded page/image caches
lazy thumbnails
incremental semantic events
event coalescing
bounded channels
Compose stable-key/recomposition architecture
Dioxus localized rerender architecture
JNI handle-based media/file boundaries
background task prioritization
memory-pressure handling
performance diagnostics
large synthetic benchmark fixtures
```

Defer:

```text
aggressive predictive prefetch
complex ML-based cache policy
unbounded offline full-corpus preloading
full-resolution media predecode
```

---

# 274. Definition of Done

UI/UX Part 26 is complete when:

- no major list requires loading the entire dataset into UI memory
- cursor paging is used for mutable large collections
- all virtualized collections use stable IDs rather than indexes
- message history supports prepend/append/eviction while preserving logical scroll anchor
- search supports request cancellation and stale-result rejection
- file/media grids use lazy bounded thumbnails and never decode originals for list display
- high-frequency progress/presence/typing/call metrics are coalesced and throttled
- UI event channels are bounded with explicit priority/drop/coalescing rules
- snapshot + incremental-event architecture can recover from stream gaps by resnapshotting
- Compose recomposition is feature/local-state scoped
- Dioxus rerenders are localized and large collections virtualized
- JNI/IPC boundaries avoid giant byte arrays and raw media frames
- background work yields to calls, emergency, security, messages, and visible UI
- UI caches are bounded and respond to memory pressure
- multi-window reuses shared heavy resources while keeping viewport state local
- accessibility semantics continue to work with virtualization
- performance budgets, diagnostics, stress fixtures, leak tests, and regression gates are defined
- the architecture is designed for millions of messages/files rather than demo-scale datasets

---

# 275. Final Architecture

```text
                     LARGE LOCAL DATA
                           │
                           ▼
                    Rust Query Layer
                           │
              Cursor Pages / Projections
                           │
                           ▼
                Presentation Data Window
                           │
          ┌────────────────┴────────────────┐
          │                                 │
      Dioxus                            Compose
      Desktop                           Android
          │                                 │
   Virtual Lists/Grids             LazyColumn/LazyGrid
   Stable Keys                     Stable Keys
   Bounded Signals                 Scoped StateFlow
          │                                 │
          └────────────────┬────────────────┘
                           │
                  Visible Content Only
```

Event path:

```text
Low-Level Runtime Events
          │
          ▼
Rust Semantic Coalescing
          │
          ▼
Bounded UI Event Stream
          │
          ▼
Only affected visible state updates
```

---

# 276. Final Principle

Performance should come from architecture, not late-stage micro-optimization.

The correct model is:

```text
indexed queries
+
cursor paging
+
stable IDs
+
virtualized rendering
+
bounded caches
+
incremental semantic events
+
backpressure
```

not:

```text
load everything
→ clone everything
→ rerender everything
→ optimize after users complain
```

This keeps Dioxus desktop and Android Compose responsive as local history, files, search indexes, plugins, and long-running usage grow from thousands to millions of records.
