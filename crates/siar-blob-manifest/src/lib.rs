#![forbid(unsafe_code)]

//! siar-blob-manifest: a first slice of "Part 05 — Robust File/Blob
//! Subsystem Architecture" (the fifth of the architecture documents
//! supplied so far). This workspace already has attachment *metadata*
//! shapes (`siar_domain::attachment::{AttachmentReference, MediaType,
//! BlobSize}`) and a working single-shot 1:1 attachment send/fetch
//! path (`siar-messaging::MessageService::send_attachment`/
//! `fetch_attachment`, wired all the way to `apps/android`) — but
//! nothing implementing §15's actual chunked manifest, §26's transfer
//! state machine, or §29's resume bitmap. This crate is that.
//!
//! ## Scope: §207 "Implementation Phases" 1, plus real slices of 2/3/5
//!
//! - [`ids`] — §5-8 `BlobId`/`ChunkHash`/`LogicalAttachmentId`/
//!   `ManifestId`, §19 `BlobEncryptionKey`'s type shape.
//!   [`ids::BlobId::from_ciphertext`]/[`ids::ChunkHash::from_ciphertext_chunk`]
//!   make §7's own recommended choice ("encrypt first, then
//!   content-address the ciphertext") the *only* way to construct
//!   these types, not just a documented convention a caller could
//!   ignore.
//! - [`descriptor`] — §9 `BlobDescriptor`, §10 `FileMetadata` (with a
//!   real bounded [`descriptor::FileName`], not a bare `String`), §18/
//!   §21 `EncryptionDescriptor`/`EncryptionAlgorithm` (naming
//!   `ChaCha20Poly1305` specifically — this workspace's already-
//!   established AEAD choice, not a new one introduced here).
//! - [`chunking`] — §11-13: real fixed-size chunking (§12's own v1
//!   recommendation over content-defined chunking) plus a size-class
//!   heuristic for §13's small/medium/large chunk-size guidance.
//! - [`manifest`] — §15 `BlobManifest`/`ChunkDescriptor` PLUS §16's
//!   limits actually enforced at construction time
//!   ([`manifest::build_manifest`] rejects an oversized file/manifest/
//!   chunk-count before allocating anything for it), not just typed
//!   and left unchecked.
//! - [`verify`] — §14's own stated benefit made real: per-chunk hash
//!   verification before a whole blob is complete, plus whole-blob
//!   verification that catches a manifest whose `blob_id` was forged
//!   independent of its per-chunk hashes.
//! - [`encryption`] — §18-19: real now, not just a type shape — see
//!   [`descriptor::EncryptionDescriptor`]'s own doc comment for how the
//!   real implementation corrected an earlier, more complicated
//!   per-chunk-nonce design this crate started with. Whole-blob AEAD
//!   ([`encryption::encrypt_blob`]/[`encryption::decrypt_blob`],
//!   mirroring — not importing — `siar_crypto::attachment`'s exact
//!   pattern), with a dedicated end-to-end test running this crate's
//!   own encrypt → chunk/manifest → verify → decrypt pipeline together,
//!   not each module tested only in isolation.
//! - [`resume`] — §29 `ResumeBitmap` + §30 range-based resume
//!   (`missing_ranges` returns contiguous `[start, end)` runs, not one
//!   entry per missing chunk).
//! - [`transfer_state`] — §26's named states as a real state machine
//!   (`TransferState::transition`) that rejects illegal transitions
//!   instead of a bare enum a caller could set to anything.
//! - [`events`] — `04-offline-event-log-architecture.md` §34 "File
//!   Events", §92 Phase 3's third domain (after identity and
//!   messaging — see that document's own crate for the other two).
//!   Nine named events, a new [`ids::TransferId`] this module needed
//!   and didn't have, and the same "construct only, never append"
//!   split this crate's own state machine already keeps between
//!   deciding and recording.
//! - [`metadata_privacy`] — §22 "Metadata Privacy": a real
//!   [`metadata_privacy::MetadataSensitivity`] classification (public
//!   transport / encrypted application / local-only) with this crate's
//!   own default assignment for every field [`descriptor::FileMetadata`]
//!   actually has, plus [`metadata_privacy::public_transport_view`] as
//!   the one place that policy is enforced in code rather than left as
//!   a convention.
//! - [`attachment_reference`] — §23 "Message Attachment Reference":
//!   [`attachment_reference::AttachmentReference`] pairing a
//!   [`descriptor::BlobDescriptor`] with either a visible or
//!   already-sealed [`attachment_reference::AttachmentMetadata`],
//!   structurally incapable of carrying file bytes. Its own doc
//!   comment also covers §24 "File-Only Transfer" — a structural
//!   property (no dependency on any conversation/message concept
//!   anywhere in this crate), not a type of its own.
//! - §25 "Transfer Identity" — already real: [`ids::TransferId`],
//!   built for §34's events before this round, is exactly what §25
//!   asks for ("a transfer is distinct from a blob; same blob may have
//!   multiple transfers/recipients/retries") — no new code needed,
//!   named here so the section isn't miscounted as untouched.
//! - [`transfer_record`] — §27 "Transfer Record":
//!   [`transfer_record::TransferRecord`], field-for-field per the
//!   spec's own snippet (plus a [`transfer_record::TransferDirection`]
//!   the snippet implies but doesn't name), with
//!   [`transfer_record::TransferRecord::advance`] keeping `state` and
//!   `updated_at_millis` from ever drifting out of sync.
//! - [`transfer_journal`] — §28 "Transfer Journal": high-frequency,
//!   NON-durable operational state
//!   ([`transfer_journal::TransferJournal`] — chunk bitmap, bytes
//!   verified, active path, retry count) kept deliberately separate
//!   from both [`transfer_record::TransferRecord`] and the event log —
//!   §28's own "do not append a permanent event for every packet,"
//!   made structural by this module never constructing a
//!   [`events::FileEvent`] at all.
//! - [`resume::ResumeRequest`] — §31 "Resume Protocol": the real
//!   wire-shaped request ("manifest known, missing chunks: ...") a
//!   receiver would send, built from a [`resume::ResumeBitmap`] via
//!   [`resume::ResumeRequest::from_bitmap`].
//! - [`mod@partial_availability`] — §32 "Partial Availability": the pure
//!   computation a future blob store's "safe partial-read status"
//!   needs — how many bytes from the start of a file are covered by an
//!   unbroken run of received chunks, deliberately not counting a
//!   chunk received out of order as "available" on its own.
//! - [`transfer_planner`] — §35 "Transfer Planner": destination
//!   resolution, chunk scheduling (including
//!   [`transfer_planner::resume_chunk_schedule`]'s missing-chunks-first
//!   ordering, tying directly into [`resume::ResumeBitmap`]),
//!   concurrency limits, and retry planning, gathered into a real
//!   [`transfer_planner::TransferPlan`]. Route request generation
//!   itself stays Part 03's own job — this module exposes only
//!   [`transfer_planner::RouteRequirements`], the sizing data a router
//!   needs, rather than depending on `siar-routing-policy`. Its own doc
//!   comment also covers §36 "Transport Independence" — a structural
//!   property (this crate has never depended on any transport crate),
//!   not a type of its own.
//! - [`file_offer`] — §44 "File Offer Protocol":
//!   [`file_offer::FileOffer`] and the receiver's own
//!   [`file_offer::FileOfferDecision`] (`Accept`/`Reject`/`Defer`/
//!   `MetadataOnly`).
//! - [`auto_accept`] — §45 "Auto-Accept Policy":
//!   [`auto_accept::AutoAcceptPolicy`]'s five named dimensions enforced
//!   by [`auto_accept::evaluate_auto_accept`], a pure function from
//!   policy plus caller-supplied [`auto_accept::AutoAcceptContext`] to
//!   an optional decision — `None` means "no automatic answer," not
//!   "reject," since an auto-accept policy has nothing useful to say
//!   about the other three decisions.
//! - [`transfer_authorization`] — §46 "Transfer Authorization":
//!   [`transfer_authorization::authorize_transfer`] checks peer
//!   authorization, size policy, storage quota, and content policy in
//!   the spec's own listed order, structurally incapable of returning
//!   `Ok` without every check passing — "connection alone does not
//!   imply permission," enforced by there being no other code path.
//! - [`filename_safety`] — §53 "Temp File Naming"
//!   ([`filename_safety::temp_file_name`], built only from a
//!   [`ids::TransferId`], never a remote-supplied name), §54 "Path
//!   Traversal Protection" (enforced on
//!   [`descriptor::FileName`] itself by omission — see that type's own
//!   doc comment — not a function in this module), §55 "Filename
//!   Sanitization" ([`filename_safety::sanitize_for_export`] and
//!   [`filename_safety::resolve_export_collision`], the spec's own
//!   `photo.jpg` → `photo (1).jpg` worked example, tested verbatim).
//! - [`content_sniffing`] — §56 "MIME Type Validation"
//!   ([`content_sniffing::sniff_media_type`]/
//!   [`content_sniffing::check_declared_media_type`], a real magic-byte
//!   sniffer covering every [`siar_domain::MediaType`] variant that has
//!   a signature to check), §57 "Executable Content"
//!   ([`content_sniffing::has_potentially_executable_extension`]). §58
//!   "Archive Safety" is named but given no code anywhere in this
//!   crate — the spec's own explicit instruction ("must be handled in
//!   a separate safe extraction subsystem. Blob layer itself stores
//!   opaque bytes").
//! - [`blob_references`] — §59 "Deduplication" (already true by
//!   construction — see this module's own doc comment for why it
//!   needed no function), §60 "Reference Counting"
//!   ([`blob_references::ReferenceKind`]/[`blob_references::ReferenceSet`],
//!   a real multiset, not a set — two messages referencing the same
//!   blob are two references, both needing removal), §61 "Pinning"
//!   ([`blob_references::PinReason`], deliberately a distinct
//!   vocabulary from `ReferenceKind`, not a renaming of it — see that
//!   type's own doc comment for the real difference in what each
//!   question means).
//! - [`storage_lifecycle`] — §62 "Cache vs Durable Blob"
//!   ([`storage_lifecycle::BlobClassification`]), §63 "Garbage
//!   Collection"/§64 "Grace Period"
//!   ([`storage_lifecycle::is_gc_eligible`], structurally unable to
//!   base eligibility on age alone — the function never even takes a
//!   creation timestamp as input, only how long a blob has been
//!   *unreferenced*), §65 "Cache Eviction"
//!   ([`storage_lifecycle::CacheEvictionTier`], the same
//!   `Ord`-by-declaration-order pattern
//!   `siar_dtn_bundle::eviction::EvictionTier` already established in
//!   this workspace, plus [`storage_lifecycle::may_evict`]'s explicit,
//!   named emergency-override parameter rather than a silent default),
//!   §66 "Storage Pressure"
//!   ([`storage_lifecycle::StoragePressure`]/[`storage_lifecycle::effects_for`],
//!   a deterministic escalating mapping across all four tiers).
//! - [`relay_and_dtn`] — §67 "Relay Blob Storage"
//!   ([`relay_and_dtn::RelayStorageAccounting`], structurally
//!   incapable of carrying a decryption key — "relay node does not get
//!   content keys" enforced by there being no such field to add), §68
//!   "DTN Chunk Strategy" ([`relay_and_dtn::DtnChunkPolicy`]/
//!   [`relay_and_dtn::chunks_to_send`], keeping "selected chunks of
//!   this blob" and "a separate thumbnail blob" as genuinely different
//!   cases rather than one).
//! - [`delivery_policy`] — §69 "Emergency Files"
//!   ([`delivery_policy::suggested_emergency_priority`], named
//!   honestly as this crate's own suggestion, not Part 17's
//!   authoritative decision), §70 "Multi-Device File Delivery"
//!   ([`delivery_policy::FileDeliveryTarget`]), §71 "Own-Device Blob
//!   Sync" ([`delivery_policy::OwnDeviceSyncPolicy`]), §72 "Peer
//!   Discovery of Blob Availability"
//!   ([`delivery_policy::BlobAvailabilityAdvertisement`]/
//!   [`delivery_policy::may_advertise_availability`], requiring BOTH
//!   of the spec's own named preconditions at once, not either).
//! - **§73 "Swarming" — explicitly deferred by the spec's own words**
//!   ("not necessary for initial release"); no code.
//! - [`source_trust`] — §74 "Source Selection"
//!   ([`source_trust::BlobSource`], vocabulary only — actual selection
//!   stays `siar-routing-policy`'s job), §75 "Trust of Sources"
//!   (resolved by construction — see that module's own doc comment for
//!   why [`verify::verify_chunk`]/[`verify::verify_complete_blob`] are
//!   what makes it true, not a new check), §77 "Manifest
//!   Authentication" — this round's other real finding:
//!   [`source_trust::verify_manifest_authenticity`] closes a gap that
//!   reading §76/§77 together surfaced: [`verify::verify_complete_blob`]
//!   only checks a manifest against *itself*, never against an
//!   externally-authenticated expected [`ids::BlobId`] — a wholesale
//!   swapped-and-internally-consistent manifest would pass it cleanly
//!   (proven by this round's own test), which is exactly the
//!   reorder/replace attack §77 warns about.
//! - **§78 "Merkle Tree Option" — explicitly deferred by the spec's
//!   own words** ("a flat chunk-hash manifest may be sufficient
//!   initially"); no code.
//! - **§79 "Chunk Compression"/§80 "File Compression Architecture" —
//!   deferred pending a real codec dependency decision**: the spec's
//!   own required pipeline order (plaintext → optional compression →
//!   encryption → chunking) and its two "never" rules (never compress
//!   ciphertext; compression must be content-aware/application-
//!   controlled, not automatic) are worth recording here rather than
//!   silently skipping, but building either section for real means
//!   picking and adding an actual compression crate — a real,
//!   consequential dependency choice this round doesn't make on a
//!   caller's behalf.
//!
//! ## Round 10: §98-106, §118-119, §140-141
//!
//! - [`transfer_control`] — §98 "Transfer ACK"
//!   ([`transfer_control::TransferAckKind`], four genuinely distinct
//!   states, not one flag), §99 "Chunk ACK"
//!   ([`transfer_control::ChunkAck`], reusing [`resume::ResumeBitmap`]'s
//!   own range machinery — a new [`resume::ResumeBitmap::received_ranges`]
//!   is the exact complement of the pre-existing
//!   [`resume::ResumeBitmap::missing_ranges`]), §100 "Completion ACK"
//!   ([`transfer_control::allows_transfer_completion`], true only for
//!   `BlobVerified` — the other three real ACK kinds are each
//!   insufficient on their own), §101 "Transfer Cancellation"
//!   ([`transfer_control::StagingRetentionPolicy`], the one real
//!   decision point among §101's four named actions), §102 "Pause"
//!   (given no type of its own — already exactly what
//!   [`transfer_journal::TransferJournal`] holds, see that module's own
//!   doc comment), §103 "Deferred Transfer"
//!   ([`transfer_control::DeferredReason`]), §104 "Retry" (already
//!   satisfied by [`transfer_planner::RetryPolicy`], no new work), §105
//!   "Corrupt Chunk" ([`transfer_control::on_corrupt_chunk`], real
//!   escalating state — a chunk's first corruption requests
//!   retransmission only; source penalization triggers only once a
//!   caller-supplied threshold is crossed), §106 "Corrupt Manifest"
//!   (resolved by construction — this crate has never had a
//!   partial-reconstruction path to disable).
//! - [`source_integrity`] — §118 "Source Mutation Detection"
//!   ([`source_integrity::SourceSnapshot`]/
//!   [`source_integrity::source_mutated`], any single field
//!   disagreeing is enough — deliberately not a majority-vote check),
//!   §119 "Immutable Source Snapshot" (an I/O strategy this crate has
//!   no staging layer to represent — documented as the reason
//!   `SourceSnapshot` is a fallback, not a requirement, when that
//!   strategy is used instead).
//! - [`progress`] — §140 "Progress Model"
//!   ([`progress::TransferProgress`], the spec's own literal struct,
//!   transcribed field-for-field), §141 "ETA"
//!   ([`progress::EtaEstimate`]/[`progress::estimate_eta`], a real,
//!   distinct `Unknown` outcome rather than a sentinel smuggled through
//!   a number — "do not present false precision" enforced by the type
//!   itself, not a formatting convention a caller could forget).
//!
//! ## Round 11: §91, §96, §108, §110-113
//!
//! - [`quarantine`] — §91 "Quarantine":
//!   [`quarantine::QuarantineRecord`], a deliberately separate type
//!   from [`storage_lifecycle::BlobClassification`] — quarantine isn't
//!   a fifth lifecycle tier, it's outside that system entirely, which
//!   is what "do not expose to normal applications" actually means.
//! - [`resume::ResumeBitmap`] — §96 "Chunk Bitmap Optimization", this
//!   round's real structural improvement: rewritten from `Vec<bool>`
//!   (one byte per chunk) to a hand-rolled bit-packed `Vec<u64>` (one
//!   bit per chunk — 64× less memory at the "thousands of chunks"
//!   scale §96 names), with `is_complete`/`received_count` doing
//!   word-level bulk checks instead of a per-chunk scan. Every public
//!   method's signature is unchanged, so every existing caller
//!   ([`transfer_planner`], [`transfer_control`], [`mod@partial_availability`])
//!   kept working without modification — proven by the full suite
//!   passing unchanged, not just the new bit-packing-specific tests.
//! - [`resource_policy`] — §108 "Rate Limiting"
//!   ([`resource_policy::RateLimitPolicy`], the same two-tier known/
//!   unknown-peer shape `siar-dtn-bundle`'s own `PeerQuotaPolicy`
//!   already established for storage quota, applied here to rate),
//!   §110 "Resource Policy" ([`resource_policy::FileResourcePolicy`],
//!   the spec's own literal struct), §111 "Battery Awareness"
//!   ([`resource_policy::BatteryAwareAdjustments`], honestly named as
//!   a minimal stand-in — Part 13 Battery-Aware Scheduling doesn't
//!   exist yet anywhere in this workspace), §112 "Memory Pressure"
//!   ([`resource_policy::MemoryPressureAdjustments`], "do not lose
//!   verified progress" enforced by omission — none of its four
//!   actions touch `TransferJournal::bytes_verified`), §113 "Thermal
//!   Pressure" ([`resource_policy::ThermalPressureAdjustments`]).
//!
//! ## Round 12: §92-95, §97, §107, §109, §114 — brought in for real,
//! not left as doc-only resolutions
//!
//! - [`schema_rows`] — §92 "Filesystem Permissions"
//!   ([`schema_rows::FilesystemPermissionPolicy`], a real owner-only-
//!   mode-bits value — actually applying it via `chmod` is real OS
//!   I/O this crate still doesn't perform, but the restrictive numbers
//!   themselves are no longer just prose), §93 "Database Schema"/§94
//!   "Blob Table"/§95 "Transfer Chunk Table" ([`schema_rows::BlobRow`]/
//!   [`schema_rows::BlobDerivative`]/[`schema_rows::TransferChunkRow`]/
//!   [`schema_rows::TransferSourceRow`]/[`schema_rows::BlobPinRow`] —
//!   real row-shape types for five of the spec's eight named
//!   conceptual tables; the other three already had a type elsewhere
//!   in this crate, mapped explicitly in this module's own doc
//!   comment. `BlobRow` has no path field of any kind — "do not store
//!   content path from untrusted metadata" enforced by there being
//!   nowhere to put one).
//! - [`crate::blob_references::ReferenceSet::len`] — added to support
//!   [`schema_rows::BlobRow::reference_count`] with a real number,
//!   not a placeholder.
//! - [`progress::TransferProgress::from_journal`] — §97 "Durable
//!   Progress," made a real enforcement instead of a true-by-
//!   omission claim: this is now the one sanctioned constructor that
//!   reads `verified_bytes` directly from
//!   [`transfer_journal::TransferJournal::bytes_verified`], the same
//!   field [`transfer_journal::TransferJournal::record_chunk_verified`]
//!   only advances once a chunk has actually passed verification.
//! - [`transfer_control::SourceFailureTracker`] — §107 "Multiple
//!   Sources," brought in for real: a per-[`siar_domain::DeviceId`]
//!   failure counter built on [`transfer_control::on_corrupt_chunk`],
//!   where "preserving other sources" is enforced by the `HashMap`
//!   keying itself, not by caller discipline.
//! - `tests/abuse_protection_matrix.rs` — §109 "Abuse Protection,"
//!   brought in for real as one consolidated integration test per
//!   named threat, proving each of the six already-real mitigations
//!   this crate has, rather than leaving the mapping as prose only a
//!   human would read.
//! - [`ids::BlobId::from_ciphertext_incremental`] — §114 "Hashing
//!   Performance," brought in for real: a genuinely usable incremental
//!   hashing entry point (blake3's own streaming `Hasher`) that a
//!   future streaming reader can call today, plus a test proving it
//!   produces the IDENTICAL [`ids::BlobId`] as whole-buffer
//!   [`ids::BlobId::from_ciphertext`] — §114's own real correctness
//!   requirement, checked directly rather than assumed.
//!
//! ## Round 13: the local blob store — §37-40, 47-52, 86-90, 186-188
//!
//! Until now this crate produced and verified manifests as pure values
//! and had **no store**: nothing wrote bytes anywhere. This round adds
//! one, on a real filesystem, with the staging / verification / atomic
//! publication / crash-recovery behaviour Part 05 specifies.
//!
//! - [`fs_store`] — §50 staging, §51 atomic finalization, §52 crash
//!   recovery, §49 sparse preallocation, §90 directory layout, and the
//!   filesystem implementation of [`blob_io`]'s traits.
//!   [`fs_store::FsBlobStore::begin_write_with_manifest`] verifies each
//!   chunk against its own hash as it arrives; `finalize` stream-hashes
//!   the whole file through a bounded buffer and only then renames it
//!   into `objects/` — an incomplete or unverified object is **never
//!   visible**. A blob that fails its identity check is moved to
//!   `quarantine/`. [`fs_store::FsBlobStore::recover`] re-hashes every
//!   staged chunk against the manifest rather than believing the file.
//! - [`blob_io`] — §86 `BlobReader`, §87 `BlobWriter`, §88 `BlobStore`.
//!   Native async-fn-in-trait with associated `Reader`/`Writer` types
//!   instead of the spec's `Box<dyn …>` (an `impl Future` trait is not
//!   object-safe; boxing would put an allocation on the hot path).
//! - [`byte_range`] — §86's `ByteRange`, validated with checked
//!   arithmetic; reads are capped
//!   ([`blob_io::MAX_READ_RANGE_BYTES`]) so there is no way to ask for a
//!   whole object as one `Bytes`.
//! - [`quota`] — §47 (the store-level quotas) and §48: space is
//!   *reserved* against the declared size before the first byte lands,
//!   so "accept 20 GB then fail at 99%" cannot happen. Per-peer and
//!   daily budgets stay in [`resource_policy`]/[`auto_accept`], which
//!   decide whether to *start* a transfer.
//! - [`metadata_store`] — §89: rows ([`schema_rows::BlobRow`]) behind a
//!   trait, in memory or on `stoolap`. Files are the truth;
//!   [`fs_store::FsBlobStore::reconcile_metadata`] rebuilds rows from
//!   them (after re-hashing) and drops rows whose file is gone.
//! - [`buffer_pool`] — §39: a fixed number of fixed-size buffers
//!   allocated once; `acquire` *blocks* when empty, which is §40's
//!   backpressure. One pool per §13 size class.
//! - [`bounded_io`] — §37/§40: the sender pipeline (`file → bounded
//!   queue → consumer`), peak memory `max_buffers × buffer_size`
//!   whatever the file size; a consumer error stops the reader.
//! - [`store_error`] — §186 closed error enum, §187
//!   [`store_error::Recoverability`] (retryable / caller error / refetch
//!   / requires repair / policy denied), §188 no `anyhow`. Errors carry
//!   the `io::ErrorKind` and operation, never a path.
//!
//! §38 "Low-Copy Data Path" is only partly met: the API's currency is
//! `bytes::Bytes`, and the writer takes chunks by reference without
//! copying, but a [`blob_io::BlobStream`] piece is allocated per read
//! rather than recycled through a [`buffer_pool::BufferPool`].
//!
//! `tests/fs_store.rs` drives all of this against real files: out-of-
//! order writes, torn/corrupted staging after a simulated crash,
//! quarantine, quota refusal, dedup, permissions (`0700`/`0600`),
//! sparse allocation, and a file streamed sender→receiver without ever
//! being buffered whole.
//!
//! ## Round 14: references, GC, compression, derivatives — §79-85, 148-157
//!
//! **References and garbage collection (§148-157).**
//!
//! - [`reference_store`] — §148 [`reference_store::BlobReference`] as a
//!   *set* of `(namespace, object, blob, role)` tuples (so two messages
//!   sharing a blob are two references, and one message re-asserting its
//!   own is a no-op, not a count of two); §149 atomic batches, plus
//!   `StoolapReferenceStore::apply_in_transaction` so a domain commits
//!   its own row and the reference in **one** transaction (tested:
//!   both survive a commit, neither survives a rollback); §155
//!   [`reference_store::release_object_references`] reports which blobs
//!   became unreferenced but deletes nothing; §156
//!   [`reference_store::disappearing_release_at`]; §157 the two meanings
//!   of "save" ([`reference_store::SaveSemantics`]).
//! - [`gc`] — §151 the mark-and-sweep: nothing is deleted when it first
//!   becomes unreferenced, only after a grace period, re-checked at the
//!   last instant; §150 the cached `reference_count` is *repaired* from
//!   the real references every sweep; `Cache`/`Relay` blobs, pinned
//!   blobs and blobs mid-transfer are never collected. §152/§153
//!   [`gc::sweep_staging`] and [`gc::reclaim_staging`] clean orphaned
//!   and partial transfers (resumable ones are kept until their
//!   [`expiry::TransferExpiry`]; storage pressure takes orphans first).
//! - [`expiry`] — §154 four *distinct types* (offer / transfer / relay /
//!   cache) with no conversion between them.
//!
//! **Compression and derivatives (§79-85).**
//!
//! - [`compression`] — §79-81. The long-pending codec decision is made
//!   and recorded: **DEFLATE via `flate2`'s pure-Rust backend**, chosen
//!   to keep this crate free of C and `unsafe`; the trade-off (Zstandard
//!   would compress better) is isolated behind a one-byte frame tag.
//!   The pipeline order is fixed and explicit
//!   ([`compression::PIPELINE_ORDER`]: compress → encrypt → chunk →
//!   address); compression is application-controlled and content-aware
//!   (refuses already-compressed media and containers, tiny inputs, and
//!   anything — ciphertext included — that does not shrink on a trial
//!   sample). Decompression is bomb-resistant: streaming, output cap,
//!   ratio cap, declared-length enforcement, trailing-data rejection,
//!   all tested with a real 128 MiB bomb that is stopped early. A bug
//!   caught while writing it — trailing bytes were silently accepted
//!   because `flate2`'s `read` decoder buffers ahead — is fixed by
//!   owning the buffer (`bufread`).
//! - [`derivatives`] — §82-85. Relationships are metadata
//!   ([`derivatives::DerivativeIndex`], in memory or `stoolap`) and can
//!   never alter a `BlobId`; a derivative may have several sources
//!   (content addressing makes identical thumbnails one blob);
//!   [`derivatives::choose_representation`] picks the smallest adequate
//!   representation for a budget and says when it had to degrade;
//!   §84 [`derivatives::VoiceNoteMeta`] is strictly bounded; §85
//!   [`derivatives::channel_for_attachment`] sends *every* attachment
//!   media type (video included) to the blob path, never to realtime.
//!   `DerivativeKind::Waveform` was added (appended, so existing
//!   encodings are unchanged).
//!
//! The receive-side wiring lives in `siar-file-transfer-service`'s
//! `transfer_engine` (see that crate), which also added
//! `FileTransferService::restore_from_log`.
//!
//! Every module is covered by tests exercising real bytes/hashes/state
//! transitions/ciphertext — including a tamper-detection test for
//! per-chunk verification, whole-blob verification, AND now AEAD
//! decryption itself, not just the happy path.
//!
//! ## What's explicitly NOT here
//!
//! - **No key exchange / key distribution.** [`encryption::generate_blob_key`]
//!   produces a key; nothing here gets that key to a recipient. In this
//!   workspace's existing `siar-messaging::MessageService::send_attachment`
//!   flow that's the message envelope's job (per-message E2EE already
//!   covers it) — a real integration point, not attempted from this
//!   crate.
//! - **Local store: real (round 13) and, as of round 14, wired into
//!   `siar-file-transfer-service`** (its `transfer_engine`), with
//!   references and GC ([`reference_store`], [`gc`]). Still not wired:
//!   `siar-messaging` does not call any of it, nothing *schedules*
//!   [`gc::sweep`]/[`gc::sweep_staging`] (they are functions awaiting a
//!   caller on a timer), and the engine covers the **receive** side
//!   only — there is no send-side engine reading a file through
//!   [`bounded_io`] into a live transport.
//! - **No transfer protocol wire messages.** §207 Phase 4 (offer/
//!   accept/manifest/chunk-request/ACK/complete as actual
//!   `siar-protocol` envelope kinds) — [`transfer_state::TransferState`]
//!   models the state machine an implementation of that protocol would
//!   drive, but doesn't define the wire messages themselves.
//! - **No transport/routing integration.** §207 Phase 6 — nothing here
//!   touches `siar-transport`, `siar-routing`, or the new
//!   `siar-routing-policy` from this same workspace, though a real
//!   integration would plug `RoutePlan`/candidate-path selection in
//!   exactly where a transfer's `TransferState::InProgress` chunk
//!   requests get dispatched.
//! - **No resource management wired to a live loop.** §207 Phase 7 —
//!   quotas, storage pressure, and eviction now exist as real, tested
//!   policy (see [`storage_lifecycle`] above), but nothing calls them
//!   from an actual scheduled task or `siar-routing-policy`'s own
//!   `RetryPolicy`/backoff — the decision logic is real, the live
//!   wiring isn't.
//! - **Progressive images/video (§33-34), manifest hierarchy for huge
//!   files (§17, explicitly "not required initially" per the spec's own
//!   text), parallelism/adaptive concurrency (§41-43), thumbnail
//!   *generation* (§83 — needs an image codec; only the policy skeleton
//!   exists), §115-117/§120-139/
//!   §142+ except where named above** — still a remaining slice of a
//!   210-section document, though a shrinking one. §20 "Chunk Nonces"
//!   and §21 "Encryption Metadata" are resolved, not open — see
//!   [`descriptor::EncryptionDescriptor`]'s own doc comment: whole-blob
//!   AEAD means there was never a per-chunk nonce to derive.

pub mod attachment_reference;
pub mod auto_accept;
pub mod blob_io;
pub mod blob_references;
pub mod bounded_io;
pub mod buffer_pool;
pub mod byte_range;
pub mod chunking;
pub mod compression;
pub mod content_sniffing;
pub mod delivery_policy;
pub mod derivatives;
pub mod descriptor;
pub mod encryption;
pub mod events;
pub mod expiry;
pub mod file_offer;
pub mod filename_safety;
pub mod fs_store;
pub mod gc;
pub mod ids;
pub mod limits;
pub mod manifest;
pub mod metadata_encryption;
pub mod metadata_privacy;
pub mod metadata_store;
pub mod partial_availability;
pub mod progress;
pub mod quarantine;
pub mod quota;
pub mod reference_store;
pub mod relay_and_dtn;
pub mod resource_policy;
pub mod resume;
pub mod schema_rows;
pub mod source_integrity;
pub mod source_trust;
pub mod storage_lifecycle;
pub mod store_error;
pub mod transfer_authorization;
pub mod transfer_control;
pub mod transfer_journal;
pub mod transfer_planner;
pub mod transfer_record;
pub mod transfer_state;
pub mod verify;

pub use attachment_reference::{AttachmentMetadata, AttachmentReference};
pub use auto_accept::{evaluate_auto_accept, AutoAcceptContext, AutoAcceptPolicy};
pub use blob_io::{BlobReader, BlobStat, BlobStore, BlobStream, BlobWriter};
pub use blob_references::{PinReason, ReferenceKind, ReferenceSet};
pub use buffer_pool::{BufferPool, BufferPools, PooledBuffer};
pub use byte_range::ByteRange;
pub use chunking::{chunk_fixed_size, ChunkSizeClass};
pub use content_sniffing::{
    check_declared_media_type, has_potentially_executable_extension, sniff_media_type, MimeTrust,
};
pub use delivery_policy::{
    may_advertise_availability, suggested_emergency_priority, BlobAvailabilityAdvertisement,
    EmergencyFileKind, EmergencyFilePriority, FileDeliveryTarget, OwnDeviceSyncPolicy,
};
pub use descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor, FileMetadata,
    FileName, FileNameTooLong,
};
pub use encryption::{decrypt_blob, encrypt_blob, generate_blob_key, EncryptionError};
pub use events::{
    decode_file_event, transfer_stream_id, FileEvent, EVENT_TYPE_BLOB_VERIFIED,
    EVENT_TYPE_TRANSFER_ACCEPTED, EVENT_TYPE_TRANSFER_CANCELLED, EVENT_TYPE_TRANSFER_COMPLETED,
    EVENT_TYPE_TRANSFER_CREATED, EVENT_TYPE_TRANSFER_FAILED, EVENT_TYPE_TRANSFER_PAUSED,
    EVENT_TYPE_TRANSFER_RESUMED, EVENT_TYPE_TRANSFER_STARTED,
};
pub use file_offer::{FileOffer, FileOfferDecision};
pub use filename_safety::{resolve_export_collision, sanitize_for_export, temp_file_name};
pub use fs_store::{
    DurabilityPolicy, FsBlobReader, FsBlobStore, FsBlobWriter, FsStoreConfig, Preallocation,
    ReconcileReport, RecoveryReport,
};
pub use ids::{BlobEncryptionKey, BlobId, ChunkHash, LogicalAttachmentId, ManifestId, TransferId};
pub use limits::ManifestLimits;
pub use manifest::{build_manifest, BlobManifest, ChunkDescriptor, ManifestError};
pub use metadata_encryption::{decrypt_file_metadata, encrypt_file_metadata};
pub use metadata_privacy::{
    default_sensitivity, public_transport_view, MetadataField, MetadataSensitivity,
    PublicTransportMetadata,
};
pub use metadata_store::{BlobMetadataStore, InMemoryBlobMetadata, StoolapBlobMetadata};
pub use partial_availability::{partial_availability, PartialAvailability};
pub use progress::{estimate_eta, EtaEstimate, TransferProgress};
pub use quarantine::{QuarantineReason, QuarantineRecord};
pub use quota::{QuotaLedger, Reservation, StoreLimits};
pub use reference_store::{
    BlobReference, BlobRole, InMemoryReferenceStore, ObjectId, RefOp, ReferenceStore,
    StoolapReferenceStore,
};
pub use relay_and_dtn::{chunks_to_send, DtnChunkPolicy, RelayStorageAccounting};
pub use resource_policy::{
    BatteryAwareAdjustments, FileResourcePolicy, MemoryPressureAdjustments, RateLimitPolicy,
    RateLimits, ThermalPressureAdjustments,
};
pub use resume::{ResumeBitmap, ResumeRequest};
pub use schema_rows::{
    BlobDerivative, BlobLifecycleState, BlobPinRow, BlobRow, DerivativeKind,
    FilesystemPermissionPolicy, TransferChunkRow, TransferSourceRow,
};
pub use source_integrity::{source_mutated, SourceSnapshot};
pub use source_trust::{verify_manifest_authenticity, BlobSource};
pub use storage_lifecycle::{
    effects_for, is_eviction_protected, is_gc_eligible, may_evict, BlobClassification,
    CacheEvictionTier, GcPolicy, StoragePressure, StoragePressureEffects,
};
pub use store_error::{BlobError, Recoverability};
pub use transfer_authorization::{
    authorize_transfer, AuthorizationChecks, AuthorizationDenialReason,
};
pub use transfer_control::{
    allows_transfer_completion, on_corrupt_chunk, ChunkAck, CorruptChunkAction, DeferredReason,
    SourceFailureTracker, StagingRetentionPolicy, TransferAckKind,
};
pub use transfer_journal::TransferJournal;
pub use transfer_planner::{
    plan_new_transfer, resume_chunk_schedule, route_requirements, RetryPolicy, RouteRequirements,
    TransferPlan,
};
pub use transfer_record::{TransferDirection, TransferRecord};
pub use transfer_state::{decide, DecideError, InvalidTransition, TransferEvent, TransferState};
pub use verify::{verify_chunk, verify_complete_blob};
