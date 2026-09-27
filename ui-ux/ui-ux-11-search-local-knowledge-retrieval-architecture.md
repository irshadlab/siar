# UI/UX Part 11 — Search & Local Knowledge Retrieval UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 11  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define global search, conversation search, file/contact/link discovery, local knowledge retrieval, optional semantic search, index/rebuild states, privacy, source navigation, pagination, and the Rust presentation contracts for both UIs.

---

# 1. Governing Principle

> **Search should feel global and immediate to the user while remaining local-first, privacy-preserving, source-grounded, and reconstructable from authoritative Rust state.**

Search must never become a hidden cloud copy of the user's private communication history.

---

# 2. Architectural Position

```text
Rust Authoritative Data
        │
        ├── Messages
        ├── Files
        ├── Contacts
        ├── Calls
        ├── Links
        └── Plugin Data
        │
        ▼
Rust Search / Index Layer
        │
        ├── lexical index
        ├── metadata index
        ├── optional semantic index
        └── result ranking
        │
        ▼
Search Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Search Modes

```rust
pub enum SearchMode {
    Global,
    Conversation(ConversationId),
    Contacts,
    Files,
    Links,
}
```

Future optional modes:

```text
Calls
Plugins
Knowledge
```

---

# 4. Search Domains

```rust
pub enum SearchDomain {
    Messages,
    Files,
    Contacts,
    Links,
    Calls,
    Plugins,
}
```

---

# 5. Global Search

Searches all locally available, authorized content:

```text
Messages
Files
Contacts
Links
Calls
Plugin data
```

The result set is filtered by Rust authorization rules before it reaches either UI.

---

# 6. Conversation Search

Scoped to one conversation.

Prioritizes:

```text
messages
attachments
links
```

and supports jump-to-message.

---

# 7. Inbox Filter Is Not Full Search

Part 04 inbox filtering only filters conversation summaries.

Part 11 full search queries durable indexed content.

---

# 8. Search Entry Points

Desktop:

```text
Ctrl/Cmd+K       Command palette / navigation
Ctrl/Cmd+Shift+F Global content search
Ctrl/Cmd+F       Current conversation search
```

Android:

```text
Search icon
Dedicated Search destination
Conversation app-bar search
```

---

# 9. Query Model

```rust
pub struct SearchQuery {
    pub text: String,
    pub scope: SearchScope,
    pub filters: SearchFilters,
    pub mode: SearchQueryMode,
}
```

---

# 10. Query Modes

```rust
pub enum SearchQueryMode {
    Lexical,
    Semantic,
    Hybrid,
}
```

Recommended v1 default:

```text
Lexical
```

Semantic/hybrid can be added later behind the same presentation contract.

---

# 11. Search Scope

```rust
pub enum SearchScope {
    Global,
    Conversation(ConversationId),
    Contact(AccountId),
    Group(GroupId),
    Domain(SearchDomain),
}
```

---

# 12. Filters

Recommended:

```text
Sender
Conversation
Date
File type
Has attachment
Unread
Message type
```

Advanced later:

```text
Exact phrase
Before
After
Mentions me
From me
Has link
```

---

# 13. Desktop Global Search Layout

```text
+-----------------------------------------------------------+
| Search [____________________________________] Filters     |
+-----------------------+-----------------------------------+
| Categories            | Results                           |
| All                   |                                   |
| Messages              |                                   |
| Files                 |                                   |
| Contacts              |                                   |
| Links                 |                                   |
| Calls                 |                                   |
+-----------------------+-----------------------------------+
| Optional Preview / Source Context                         |
+-----------------------------------------------------------+
```

---

# 14. Android Global Search Layout

```text
Search App Bar
Filter Chips
Result Sections
LazyColumn
```

On tablets/foldables:

```text
Result List
+
Source Preview
```

when space allows.

---

# 15. Search Debounce

Recommended:

```text
Desktop: 100–200 ms
Android: 150–300 ms
```

Tune through measurement.

---

# 16. Cancellation

Each request has a stable identity.

```rust
pub struct SearchRequestId(pub u64);
```

When the query changes:

```text
cancel/ignore stale request
```

Only the latest request may update visible results.

---

# 17. Search Result Types

```rust
pub enum SearchResultView {
    Message(MessageSearchResultView),
    File(FileSearchResultView),
    Contact(ContactSearchResultView),
    Link(LinkSearchResultView),
    Call(CallSearchResultView),
    Plugin(PluginSearchResultView),
}
```

---

# 18. Message Search Result

```rust
pub struct MessageSearchResultView {
    pub message: MessageId,
    pub conversation: ConversationId,
    pub sender: SenderView,
    pub snippet: SearchSnippet,
    pub sent_at: Timestamp,
    pub rank: SearchRank,
}
```

---

# 19. Result Snippets

Show a concise local snippet with matching terms emphasized.

Do not render an entire long message just because it matched.

---

# 20. Result Highlighting

Use visual emphasis plus normal readable text.

Do not rely on color alone.

---

# 21. File Search Result

Show:

```text
filename
file type
conversation
sender
date
content snippet if indexed
availability state if useful
```

---

# 22. Contact Search Result

Show:

```text
name
relationship
verification state
```

---

# 23. Link Search Result

Show:

```text
domain
title if known
conversation
sender
date
```

---

# 24. Call Search Result

If call history is indexed:

```text
contact/group
date
duration
missed/answered
```

---

# 25. Plugin Search Result

Always identify source provider:

```text
From Notes plugin
```

Plugin search remains permissioned and namespaced.

---

# 26. Result Grouping

Global search can show:

```text
Messages
Files
Contacts
Links
Calls
```

with a few best results from each.

Selecting a category opens the complete paged view.

---

# 27. Ranking

Rust search engine owns relevance and ranking.

UI must not invent a second ranking algorithm.

---

# 28. Sort Options

Default:

```text
Relevance
```

Optional:

```text
Newest
Oldest
```

---

# 29. Jump to Source Message

Selecting a message result:

```text
SearchResult
→ MessageId
→ request page around MessageId
→ typed conversation navigation
→ temporary highlight
```

---

# 30. Search Outside Loaded Timeline

Use the Part 05 centered page API:

```rust
pub struct MessageAroundRequest {
    pub conversation: ConversationId,
    pub anchor: MessageId,
    pub before: usize,
    pub after: usize,
}
```

---

# 31. Search Navigation Preservation

Returning to search should restore:

```text
query
filters
scroll position
selected result
```

---

# 32. Desktop Preview Pane

Wide desktop may preview:

```text
message context
file metadata
contact profile summary
```

without leaving search.

Preview should not accidentally mark a message read unless actual Part 30 read semantics are satisfied.

---

# 33. Android Search Navigation

Phone usually navigates to the source.

Tablet/foldable may use list/detail preview.

---

# 34. Search History

Optional local-only feature.

Store recent query strings:

```text
device-local
```

with:

```text
Clear history
Disable history
```

---

# 35. Strict Privacy Mode

Allow:

```text
Do not save recent searches
```

---

# 36. No Remote Autocomplete

Hard default:

```text
query text does not go to a remote autocomplete/search server
```

---

# 37. Local Suggestions

Safe local suggestions may include:

```text
recent searches
recent contacts
recent conversations
```

---

# 38. Lexical Search

Baseline search capabilities:

```text
word match
phrase match
prefix
filters
Unicode normalization
```

---

# 39. Fuzzy Search

Useful for:

```text
contact names
filenames
```

Use bounded fuzziness.

---

# 40. Advanced Query Syntax

Desktop power users may later support:

```text
from:alice
in:project
has:file
before:2026-08-01
"exact phrase"
```

Android should expose equivalent filters visually rather than requiring syntax.

---

# 41. Query Parsing

Rust parses and validates query syntax.

UI can expose a builder/chips over the same semantics.

---

# 42. Invalid Query

Show inline:

```text
Invalid date filter
Unknown search filter
```

Do not show a modal.

---

# 43. Semantic Search

Optional later.

Must remain local by default.

---

# 44. Hybrid Search

Combines:

```text
lexical
metadata
semantic similarity
```

Rust owns fusion/ranking.

---

# 45. Semantic UX

Do not force users to understand embeddings.

A future product may simply label:

```text
Search by meaning
```

or transparently use hybrid search.

---

# 46. Embedding Privacy

Embeddings are derived private data.

Store locally by default.

---

# 47. External AI

If a user explicitly configures an external provider:

```text
provider
data scope
retrieved snippets
```

must be explicit.

Never silently upload the local search corpus.

---

# 48. Local Knowledge Retrieval

Search can power:

```text
local assistant
question answering
summarization
context retrieval
```

but retrieval must be source-grounded.

---

# 49. Knowledge Chunk

```rust
pub struct KnowledgeChunkView {
    pub source_id: SearchSourceId,
    pub source_type: SearchDomain,
    pub snippet: String,
    pub conversation: Option<ConversationId>,
    pub message: Option<MessageId>,
    pub file: Option<BlobId>,
}
```

---

# 50. Retrieval Principle

A local assistant only receives actual retrieved source chunks.

No fabricated "memory" documents.

---

# 51. Source-Linked Answers

Assistant answers should expose source links to:

```text
message
file
conversation
contact
```

where relevant.

---

# 52. Bounded Retrieval

Never retrieve entire history blindly.

Use:

```text
max chunks
max tokens/bytes
scope limits
```

---

# 53. Search Index States

```rust
pub enum SearchIndexState {
    Ready,
    Building,
    Updating,
    Stale,
    Corrupt,
    Disabled,
}
```

---

# 54. Ready

Normal state.

---

# 55. Building

Initial/rebuild state.

Messaging remains fully usable.

---

# 56. Updating

Normal background incremental indexing.

Usually invisible.

---

# 57. Stale

Results may temporarily be incomplete.

Only surface if user needs to know.

---

# 58. Corrupt

Rust quarantines/rebuilds index.

UI:

```text
Search index is being repaired
```

---

# 59. Disabled

Search screen explains:

```text
Search indexing is disabled
```

with Enable action if policy allows.

---

# 60. Search During Index Build

If partial querying is supported:

```text
show available results
+
Indexing…
```

Otherwise show progress without blocking rest of app.

---

# 61. Index Rebuild

Advanced/settings action:

```text
Rebuild Search Index
```

---

# 62. Rebuild Progress

```rust
pub struct SearchIndexProgress {
    pub completed: u64,
    pub total_estimate: Option<u64>,
}
```

---

# 63. Search Empty State

Empty query:

```text
Search messages, files, contacts, and links
```

---

# 64. No Results

```text
No results for "..."
```

Potential suggestions:

```text
Remove filters
Search all conversations
Check spelling
```

---

# 65. No Results During Build

Show both facts:

```text
No results yet
Search indexing is still in progress
```

---

# 66. Offline Search

Works against local indexes without Internet.

---

# 67. Incomplete Local History

If a device does not have all historical content, search may be incomplete.

Do not claim complete coverage.

---

# 68. Coverage Warning

When relevant:

```text
Some older content is not available on this device
```

---

# 69. Per-Device Search

Each device maintains its own index.

Indexes are not synced by default.

---

# 70. Different Results Across Devices

This can be normal if devices have different local history/file availability.

---

# 71. Trusted Archive Search

Future optional feature:

```text
Search trusted archive
```

must be explicit.

---

# 72. Federated Search

Future search may query:

```text
local core
plugins
trusted archive
```

with per-provider budgets/timeouts.

---

# 73. Provider Failure

A failing plugin provider does not fail whole search.

UI can show:

```text
Some results are unavailable
```

---

# 74. Provider Permissions

Plugins search only their own namespace unless granted broader permission.

---

# 75. Pagination

Use cursor-based paging.

```rust
pub struct SearchResultPage {
    pub request_id: SearchRequestId,
    pub results: Vec<SearchResultView>,
    pub next_cursor: Option<SearchCursor>,
    pub total_estimate: Option<u64>,
}
```

---

# 76. Exact Result Count

Optional.

Do not block expensive queries to compute exact counts for huge datasets.

---

# 77. Stable Search Source Identity

```rust
pub enum SearchSourceId {
    Message(MessageId),
    File(BlobId),
    Contact(AccountId),
    Link(LinkId),
    Call(CallId),
    Plugin {
        plugin: PluginId,
        item: PluginItemId,
    },
}
```

---

# 78. Virtualization

Required for large result sets.

Desktop:

```text
virtualized result list
```

Android:

```text
LazyColumn / LazyGrid
```

---

# 79. Result Stability

Avoid continuous result movement while user tries to click.

Batch/rerank carefully.

---

# 80. v1 Result Strategy

Recommended:

```text
short debounce
→ stable first page
```

rather than streaming multiple ranking reorder phases.

---

# 81. Conversation Search — Desktop

Can open:

```text
right-side search inspector
```

while timeline remains visible.

---

# 82. Conversation Search — Android

Top app bar enters search mode.

Results can show:

```text
list of matching messages
```

or next/previous controls over timeline.

---

# 83. Result Navigation

After jumping:

```text
Previous match
Next match
3 of 18
```

if total available.

---

# 84. Files Search Integration

Part 10 Files destination can scope:

```text
SearchDomain::Files
```

---

# 85. Contacts Search Integration

Part 08 can scope:

```text
SearchDomain::Contacts
```

---

# 86. Group Search Integration

Part 09 can scope:

```text
Conversation(GroupId)
```

or group member search using dedicated group member API.

---

# 87. Unicode & Multilingual Search

Must handle:

```text
English
Hindi
Urdu
Arabic
mixed scripts
emoji
```

without UI assumptions.

---

# 88. Bidi Rendering

Snippets with mixed RTL/LTR must render safely.

---

# 89. Stemming/Language Processing

Backend feature.

UI should not promise language-specific behavior unless supported.

---

# 90. Search Accessibility

Search field label:

```text
Search messages, files, contacts, and links
```

---

# 91. Message Result Accessibility

Example:

```text
Message from Alice in Project Group, yesterday, matching deadline
```

---

# 92. File Result Accessibility

Example:

```text
PDF timetable.pdf from School Group, 2.4 megabytes
```

---

# 93. Result Count Accessibility

Announce once when useful:

```text
18 results
```

not after every minor update.

---

# 94. Search Loading Accessibility

Only announce Searching if latency is noticeable.

---

# 95. Keyboard Focus — Desktop

Preserve selected `SearchSourceId` when results update if still present.

---

# 96. Android Keyboard Behavior

Returning from a result should not automatically reopen IME unless search field was active.

---

# 97. Filter Accessibility

All chips/buttons have explicit labels and selected state.

---

# 98. Large Font

Search chips/result rows wrap without clipping.

---

# 99. RTL

Search controls mirror correctly.

Identifiers/fingerprints keep canonical direction where necessary.

---

# 100. Reduced Motion

Search does not need large transitions.

---

# 101. Search Privacy Settings

Potential:

```text
Index message text
Index file contents
Enable semantic search
Keep search history
```

---

# 102. Recommended Defaults

Enable local lexical indexing for:

```text
messages
metadata
contacts
filenames
links
```

Optional:

```text
file content extraction
semantic index
```

---

# 103. File Content Indexing

Supported safe parsers may extract:

```text
PDF text
Markdown
plain text
office text
```

with strict limits.

---

# 104. OCR

Future optional.

Do not imply image-text search until OCR exists.

---

# 105. Audio Transcription Search

Future optional.

Must be explicit/local-first where practical.

---

# 106. Index Storage

Storage manager may show:

```text
Search index — 420 MB
Semantic index — 1.3 GB
```

---

# 107. Clear Index

Because index is derived:

```text
Clear Search Index
```

is safe.

Search can rebuild later.

---

# 108. Low Storage

Rust may discard semantic index first.

UI can explain if user opens storage management.

---

# 109. Search Locking

If app is locked, search results are hidden.

---

# 110. Logging

Never log raw search queries/snippets by default.

---

# 111. Crash Reports

Redact:

```text
query
snippet
file content
```

---

# 112. Safe Metrics

Possible:

```text
search latency
result-count bucket
index size
rebuild duration
query cancellation rate
```

No query content.

---

# 113. Performance Goal

Local lexical search should feel near-instant on typical hardware.

A useful target:

```text
first useful page within ~150 ms
```

where hardware/corpus permit.

---

# 114. Semantic Search Latency

May be slower.

UI may show:

```text
Refining results…
```

if hybrid processing is deliberately staged.

---

# 115. Background Indexing Priority

Indexing yields to:

```text
calls
message send/receive
critical file work
emergency traffic
```

---

# 116. Android Background Indexing

Respect:

```text
battery
thermal
background execution
```

through Rust scheduler/platform constraints.

---

# 117. Desktop Background Indexing

Low-priority worker.

---

# 118. Search Errors

```rust
pub enum SearchUiErrorKind {
    QueryInvalid,
    IndexUnavailable,
    IndexBuilding,
    ProviderUnavailable,
    PermissionDenied,
    Internal,
}
```

---

# 119. Error UX

Query invalid:

```text
inline correction
```

Index unavailable:

```text
Search unavailable
Rebuild Index
```

Provider unavailable:

```text
partial results remain
```

---

# 120. Permission Enforcement

Rust filters unauthorized content before producing UI results.

Never send secret result then rely on UI hiding.

---

# 121. Deleted Result

If result disappears before open:

```text
This result is no longer available
```

---

# 122. Edited Message

Timeline opens current version.

Search snippet refreshes asynchronously.

---

# 123. Removed Local File

Metadata result may remain if source message exists.

Opening can trigger download if available.

---

# 124. Left Group

Results disappear or become inaccessible according to retention/security policy.

---

# 125. Search Screen Snapshot

```rust
pub struct SearchScreenSnapshot {
    pub index_state: SearchIndexState,
    pub recent_queries: Vec<RecentSearchView>,
    pub enabled_domains: Vec<SearchDomain>,
    pub semantic_available: bool,
}
```

---

# 126. Search Presentation API

```rust
pub trait SearchPresentation {
    async fn snapshot(
        &self,
    ) -> Result<SearchScreenSnapshot, UiError>;

    async fn search(
        &self,
        request: SearchRequest,
    ) -> Result<SearchResultPage, UiError>;

    async fn next_page(
        &self,
        cursor: SearchCursor,
    ) -> Result<SearchResultPage, UiError>;

    async fn cancel(
        &self,
        request: SearchRequestId,
    ) -> Result<(), UiError>;
}
```

---

# 127. Search Request

```rust
pub struct SearchRequest {
    pub request_id: SearchRequestId,
    pub query: SearchQuery,
    pub page_size: usize,
}
```

---

# 128. Search Events

```rust
pub enum SearchUiEvent {
    IndexStateChanged(SearchIndexState),
    IndexProgressChanged(SearchIndexProgress),
    ProviderStateChanged(SearchProviderState),
}
```

---

# 129. Knowledge Retrieval API

```rust
pub trait KnowledgeRetrievalPresentation {
    async fn retrieve(
        &self,
        request: KnowledgeRetrievalRequest,
    ) -> Result<Vec<KnowledgeChunkView>, UiError>;
}
```

---

# 130. Retrieval Request

```rust
pub struct KnowledgeRetrievalRequest {
    pub query: String,
    pub scope: SearchScope,
    pub max_chunks: usize,
}
```

---

# 131. Android ViewModel

Owns:

```text
query field
active filters
selected category
search mode
scroll restoration
UiEffects
```

Rust owns:

```text
index
query execution
ranking
permissions
pagination
```

---

# 132. Dioxus Presenter

Owns:

```text
query signal
filter panel
preview pane
selection
keyboard focus
```

---

# 133. Hard UI Boundaries

Do not:

```text
query DB directly from UI
parse index files in UI
implement independent search ranking in Kotlin
upload search text remotely by default
```

---

# 134. Testing Matrix

Required:

```text
empty query
global query
conversation query
messages
files
contacts
links
filters
no results
index building
index rebuild
offline
provider failure
large corpus
deleted result
rapid query changes
```

---

# 135. Desktop Tests

Verify:

```text
keyboard shortcuts
result navigation
preview pane
jump to source
back to search
filters
pagination
```

---

# 136. Android Tests

Verify:

```text
search app bar
IME
filter chips
back navigation
source jump
process death
TalkBack
large font
RTL
```

---

# 137. Search Cancellation Test

Type rapidly:

```text
a
al
ali
alic
alice
```

Only the latest result set is rendered.

---

# 138. Index Rebuild Test

Search degrades safely while app remains usable.

---

# 139. Privacy Test

No query/snippet leaves device without an explicit configured external-provider action.

---

# 140. Plugin Permission Test

Plugin search never crosses unauthorized namespace.

---

# 141. Large Corpus Test

Test:

```text
100k messages
1M messages
many files
```

with bounded memory and paging.

---

# 142. Search Jump Test

Result deep in history correctly loads around stable `MessageId`.

---

# 143. Search History Test

Clearing history does not clear index.

---

# 144. Multi-Device Test

Different devices can legitimately show different search coverage while maintaining the same source IDs for shared content.

---

# 145. Recommended v1 Scope

Ship:

```text
global lexical search
conversation search
message results
file results
contact results
link results
sender/date/type filters
jump to source
recent searches local-only
index rebuild state
offline search
desktop keyboard search
Android search app bar/chips
```

Defer:

```text
semantic/vector search
local assistant
OCR
audio transcription
trusted archive search
complex query language
cross-device federated search
```

unless already implemented safely.

---

# 146. Definition of Done

UI/UX Part 11 is complete when:

- global and conversation search are distinct
- search is local-first and private by default
- Rust owns indexing, parsing, ranking, permissions, and paging
- the UIs never query databases or index formats directly
- result types for messages/files/contacts/links/calls/plugins are defined
- every result can navigate using stable source identities
- stale queries cannot overwrite newer results
- Building/Stale/Corrupt/Disabled index states have clear UX
- index rebuild never blocks core messaging
- large result sets are paged/virtualized
- desktop is keyboard-first and can use a preview pane
- Android uses native search app bar/chips and lifecycle-safe ViewModels
- optional semantic/local-knowledge retrieval remains local-first, bounded, and source-grounded
- external AI use is explicit
- accessibility, RTL, large font, reduced motion, and screen-reader semantics are defined
- query/snippet telemetry is prohibited by default
- Rust search and knowledge retrieval presentation APIs are specified
- offline, incomplete history, provider failure, corruption, deletion races, and large-corpus scenarios are tested

---

# 147. Final Architecture

```text
                    RUST SEARCH CORE
                          │
          ┌───────────────┼───────────────┐
          │               │               │
      Lexical Index    Metadata       Optional Semantic
          │               │               │
          └───────────────┼───────────────┘
                          │
                  Search Presentation
                    ┌─────┴─────┐
                    │           │
                 Dioxus      Compose
                    │           │
             Desktop Search  Android Search
```

Optional local knowledge path:

```text
Search/Retrieval
      │
      ▼
Bounded Source Chunks
      │
      ▼
Local Assistant
      │
      ▼
Source-linked Answer
```

There is no default path:

```text
private communication history
→ central search service
```

---

# 148. Final Principle

The search experience should make a large private communication history feel small and navigable without giving up privacy.

The correct model is:

```text
local indexing
+
stable source identities
+
fast lexical retrieval
+
clear filters
+
source navigation
+
optional bounded semantic retrieval
```

not:

```text
upload the user's communication history so search feels convenient
```

This gives both Dioxus desktop and Android Compose a fast, useful, trustworthy search experience while the Rust search/index layer remains authoritative and rebuildable.
