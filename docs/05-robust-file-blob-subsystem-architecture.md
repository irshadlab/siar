# Part 05 — Robust File / Blob Subsystem Architecture

Source spec: `sys-arch/05-robust-file-blob-subsystem-architecture.md` — 210 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**Substantially complete across 27 rounds (~172+/210 sections addressed, ~38 open or deferred).** 693 tests pass (0 failing) across `siar-crypto`, `siar-blob-manifest`, `siar-file-transfer-service`, `siar-files`, `siar-transport`, and `siar-messaging`. Clippy `-D warnings` and fmt clean. Real network verification over real QUIC / iroh endpoints completed.

## Implementing crate(s)

- `siar-blob-manifest` — types, manifest v1/v2 (`chunked_encryption`), descriptor, encryption, verification, bitmaps, planning, authorization, policies, chunking, fuzz targets.
- `siar-file-transfer-service` — local blob store, reference store + GC scheduler (`GcScheduler`), transfer engine (`TransferEngine`: receive, pause, resume, cancel, restart, crash recovery, crash injection, stoolap durable paging).
- `siar-files` — public API (`FilesService`), `SendHandle`, `IncomingOffer`, streaming send/import/export, `FairScheduler`, wrapped-key multi-recipient delivery (`send_file_to_many`, `send_file_to_many_streaming`), revocation ledger (`AccessLedger`), manifest LRU cache.
- `siar-transport` — `files/1` protocol handler (`FILES_ALPN`, `FilesProtocolHandler`, `SiarEndpoint::bind_with_files`).
- `siar-messaging` — `PeerBook`, `TransportFileChannel`, `FilesInbound`, `fetch_attachment_over_files`, `send_imported_attachment`, `FilesBlobStore`.
- `siar-crypto` — pairwise secret derivation for blob key wrapping (`comm/file-keywrap/v1` domain, `FileKeyWrapSecret`).

## Round-by-round history

1. **Baseline (pre-2026-09-01)**: §5-8 (ids: `BlobId`, `ManifestId`, `ChunkHash`, `LogicalAttachmentId`), §9-10 (`descriptor.rs`: `BlobDescriptor`, `FileMetadata`), §11-13 (`chunking.rs`: fixed-size chunking), §14 (`verify.rs`), §15-16 (`manifest.rs`: `BlobManifest`/`build_manifest`), §18-19 (`encryption.rs`: whole-blob AEAD), §26 (`transfer_state.rs`: `TransferState`/`TransferEvent`), §29-30 (`resume.rs`: `ResumeBitmap`). Plus `metadata_encryption.rs`, which is actually Part 28 §31, not one of this spec's own sections — a real cross-spec module living here because the type it encrypts (`FileMetadata`) does.
2. **Phase 3 round 2, 2026-09-19 (see `docs/04` round 3 for full detail)**: §34 File Events, `04-offline-event-log-architecture.md`'s own section, not this spec's — `events.rs`, `EventTypeId` 200-208, `FileEvent` enum lined up with `TransferState`/`TransferEvent`. New `TransferId` newtype added to `ids.rs`. 37/37 tests at the time.
3. **§9 schema-version fix, 2026-09-19 (see `docs/04` round 7)**: `decode_file_event` gained real `schema_version` checking (`FileEventDecodeError::UnsupportedVersion`/`Malformed`), `CURRENT_FILE_EVENT_SCHEMA_VERSION` constant. 38/38 tests.
4. **§56 Durability Classes, 2026-09-25 (see `docs/04`)**: per-`FileEvent`-variant `durability_class()` classification added, part of a cross-domain pass touching all five event catalogs.
5. **§22-25/§27-28/§31-32 cluster, 2026-09-26**: metadata privacy, attachment references, transfer records/journal, resume protocol, partial availability — see round 5 detail below.
6. **§35-36/§44-46 cluster, 2026-09-27**: transfer planning, file offer/auto-accept/authorization — see round 6 detail below.
7. **§53-58 cluster, 2026-09-28**: filename safety and content sniffing — see round 7 detail below.
8. **§59-66 cluster, 2026-09-28**: reference counting, pinning, and the cache/GC/storage-pressure lifecycle — see round 8 detail below.
9. **§67-80 cluster, 2026-09-28**: relay/DTN delivery policy, emergency file priority, multi-device/own-device sync, source trust, and a real manifest-authentication gap found and closed — see round 9 detail below.
10. **§98-106/§118-119/§140-141 cluster, 2026-09-29**: transfer ACK/cancellation/pause/deferred/retry/corruption handling, source mutation detection, and the progress/ETA model — see round 10 detail below.
11. **§91/§96/§108/§110-113 cluster, 2026-09-29**: quarantine, a real bit-packed `ResumeBitmap` rewrite, and rate-limit/resource/battery/memory/thermal policy — see round 11 detail below.
12. **§92-95/§97/§107/§109/§114, 2026-09-29**: reconsidered at the user's own explicit request after round 11 marked them resolvable without code — real row-shape types, a real per-source failure tracker, a real enforced progress constructor, a real consolidated abuse-protection test, and a real incremental-hashing entry point — see round 12 detail below.
13. **Round 13**: local blob store (§37, 39, 40, 47-52, 86-90, 186-188, §38 partly) — see round 13 detail below.
14. **Round 14**: §148-157 reference store + GC + staging retention (+ §150 recount), §154 typed expiries, §79-81 compression (DEFLATE/flate2) + bomb defence, §82-85 derivatives/voice/channel separation; store wired into `siar-file-transfer-service` (`transfer_engine`) — see round 14 detail below.
15. **Round 15**: §132-139 `wire` (files/1 control frames, bounded manifest frame, fixed 88-byte chunk header validated before allocation, capability negotiation §133-134), §174-185 `scheduling` (path-change plans, weighted fair scheduler with per-peer/per-app caps, metrics, diagnostics); wired `siar-file-transfer-service::protocol` (`FileReceiver`/`FileSender`) and `gc_schedule::GcScheduler` — see round 15 detail below.
16. **Round 16 (§201-205, new crate `siar-files`)**: `FilesService` public API (`send_file`, `next_offer`, `import_attachment`/`bind_to_object`/`open_attachment`, `FileSource`), `siar-messaging` integration (`send_imported_attachment`, `FilesBlobStore`) — see round 16 detail below.
17. **Round 17**: `FilesService` chunk serving through `FairScheduler` (class weights, emergency priority, round-robin), GC ownership via `maintenance(now)` / `spawn_maintenance(period)` — see round 17 detail below.
18. **Round 18 (§115-117, §120-129)**: `file_io` (async `FileSource`/`FileSink`), `plaintext_cache`, `preprocess`, `export_attachment`, streaming AEAD design finding — see round 18 detail below.
19. **Round 19 (chunked AEAD v2 + §142-147, §158-173)**: blob format v2 (`chunked_encryption`), `ManifestBuilder`, `siar-files` streaming transfer, app policy, backup, sharing, key wrapping — see round 19 detail below.
20. **Round 20 (§41-43, §191-196)**: `parallelism` (per-path ceilings, resource reductions, AIMD adaptive mode), pipelined chunk pump in `FilesService`, scheduler caps, 11 proptest properties, 32 MiB test — see round 20 detail below.
21. **Round 21 (§189-198 gaps)**: crash injection (§193), hostile network (§190) + stalled retry, Bluetooth scenario (§197), DTN/emergency scenario (§198), cargo-fuzz targets (§192) — see round 21 detail below.
22. **Round 22 (durable-store crash injection)**: `crash_durable.rs` on stoolap files, recovery of persisted states, fix for stoolap `LIMIT i64::MAX` overflow in `reconcile_metadata` and pagination support — see round 22 detail below.
23. **Round 23 (files/1 over real QUIC, and pull)**: `siar-transport` `files/1` protocol handler (`FILES_ALPN`), `siar-messaging` (`PeerBook`, `TransportFileChannel`), PULL support (`BlobRequest`, `BlobUnavailable`, `ServeGate`) — see round 23 detail below.
24. **Round 24**: offer re-send (`retry_unanswered_offers`), manifest cache, file key + name carriage in offers, first-response deadline (`fetch_first_response_millis`), clippy hygiene (`BlobToSend`) — see round 24 detail below.
25. **Round 25 (wrapped-key and multi-recipient delivery)**: `siar-crypto` `comm/file-keywrap/v1` domain, strict 60-byte `WrappedKey`, `send_file_to_many` with one import and wrapped keys per recipient — see round 25 detail below.
26. **Round 26 (streaming multi-recipient)**: `send_file_to_many_streaming` with v2 chunked AEAD and bounded memory (256 KiB max chunk read) — see round 26 detail below.
27. **Round 27 (revocation, cache pruning, deadlock fix)**: `FilesService::revoke_access`/`restore_access` with `AccessLedger`, manifest LRU cache pruning on GC, and fix for inbound cancel deadlock — see round 27 detail below.

### Round 5 (2026-09-26) — §22-25, §27-28, §31-32

Picked up the next coherent cluster of untouched sections after the baseline + event-integration work above, following §20/§21's own resolution (see "Known gaps" below) rather than treating them as still open.

- **§20 Chunk Nonces / §21 Encryption Metadata — resolved, not new work**: `descriptor::EncryptionDescriptor`'s own doc comment already explains why there's no per-chunk nonce to derive — this crate encrypts the whole blob as one AEAD unit, not per chunk. Counted here as closed by that earlier design decision, not by new code this round.
- **§22 Metadata Privacy**: new `metadata_privacy.rs` — a real `MetadataSensitivity` enum (`PublicTransport`/`EncryptedApplication`/`LocalOnly`) with a fixed default classification for every field `FileMetadata` actually has (`display_name`/`media_type` → `EncryptedApplication`; `logical_size` → `PublicTransport`; `created_at_millis` → `EncryptedApplication`), plus `public_transport_view` as the one place that split is enforced in code. Named honestly: the spec's two other example fields (dimensions, duration) have no home in `FileMetadata` yet.
- **§23 Message Attachment Reference / §24 File-Only Transfer**: new `attachment_reference.rs` — `AttachmentReference { blob: BlobDescriptor, metadata: AttachmentMetadata }`, with `AttachmentMetadata` as a real `Visible(FileMetadata)`/`Encrypted(Vec<u8>)` choice rather than always-ciphertext. §24's "file-only transfer" requirement is treated as a structural property, not a type: documented via this crate's continued zero dependency on `siar-messaging`.
- **§25 Transfer Identity — already satisfied**: `ids::TransferId` (added in round 2, above, for §34's events) already is what §25 asks for. No new code; named here so the section isn't miscounted as untouched.
- **§27 Transfer Record**: new `transfer_record.rs` — `TransferRecord` (transfer_id/blob_id/direction/peer/state/created_at_millis/updated_at_millis), field-for-field per the spec's own snippet plus a `TransferDirection` (`Incoming`/`Outgoing`) the snippet implies but doesn't name. `TransferRecord::advance` keeps `state` and `updated_at_millis` from drifting out of sync, leaving the record untouched on an illegal transition.
- **§28 Transfer Journal**: new `transfer_journal.rs` — `TransferJournal` (chunk bitmap reusing `ResumeBitmap`, bytes verified, active path, retry count), deliberately never constructing a `FileEvent` — high-frequency operational state kept separate from the durable event log, per the section's own "do not append a permanent event for every packet."
- **§31 Resume Protocol**: extended `resume.rs` with `ResumeRequest { manifest_id, missing_ranges }` and `ResumeRequest::from_bitmap`, reusing §30's own range form rather than one entry per missing chunk.
- **§32 Partial Availability**: new `partial_availability.rs` — `partial_availability(manifest, bitmap) -> PartialAvailability`, computing how many bytes from the start of a file are covered by an unbroken run of received chunks (a chunk received out of order deliberately does not count, since it isn't safely readable on its own).

**Verification**: 22 new tests, 60/60 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean (one auto-format pass applied). `cargo doc` clean (caught and fixed one real broken intra-doc link: `partial_availability` was ambiguous as both a function and a module name). Verified against the real uploaded `Cargo.lock` with rustc/cargo/clippy/rustfmt 1.91.1, installed fresh this round (the sandbox that ran this round started with no Rust toolchain at all). Dependent crates re-verified with full test suites, zero regressions: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 56/56, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6.

### Round 6 (2026-09-27) — §35-36, §44-46

Picked up the next cluster in section order: transfer planning/transport independence, then the offer/auto-accept/authorization policy group that follows it, skipping over §37-43 (streaming I/O, low-copy data paths, buffer pools, parallelism — all real subsystems needing actual I/O/concurrency infrastructure this crate deliberately doesn't have) to land on sections that, like everything else here, are pure types and decisions.

- **§35 Transfer Planner**: new `transfer_planner.rs` — `TransferPlan` (destination/chunk_schedule/concurrency_limit/retry_policy), `RetryPolicy::backoff_millis` (exponential, doubling per attempt), and two real chunk-scheduling strategies: `plan_new_transfer` (ascending order for a fresh transfer) and `resume_chunk_schedule` (missing chunks first, built directly from `ResumeBitmap` — added a small `ResumeBitmap::chunk_count()` accessor to support it). Route request generation itself stayed out, per the spec's own "consumes Part 03 routing decisions" — `RouteRequirements { total_size, chunk_count }` gives a future router the sizing data it needs without this crate taking a `siar-routing-policy` dependency.
- **§36 Transport Independence — resolved, not new work**: documented as a structural property, same treatment as §24 — this crate has never depended on any transport crate, and `TransferPlan` carries nothing transport-specific.
- **§44 File Offer Protocol**: new `file_offer.rs` — `FileOffer` (transfer_id/blob/encrypted_metadata/expiry_millis) and `FileOfferDecision` (`Accept`/`Reject`/`Defer`/`MetadataOnly`), plus `FileOffer::is_expired`.
- **§45 Auto-Accept Policy**: new `auto_accept.rs` — `AutoAcceptPolicy`'s five named dimensions (contacts-only, max size, Wi-Fi-only, allowed media types, emergency thumbnails) enforced by `evaluate_auto_accept(policy, context) -> Option<FileOfferDecision>`, where `None` means "no automatic answer" rather than a disguised rejection. Emergency thumbnails are a real bypass of the other AND-conditions when the policy allows them, not one more condition to satisfy — matches an SOS report's own urgency.
- **§46 Transfer Authorization**: new `transfer_authorization.rs` — `authorize_transfer(AuthorizationChecks) -> Result<(), AuthorizationDenialReason>`, checking peer authorization/size/quota/content policy in the spec's own listed order, structurally unable to return `Ok` on an all-false input ("connection alone does not imply permission," tested directly).

**Verification**: 19 new tests, 79/79 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc` clean. Dependent crates re-verified with full test suites in the same round this time (not deferred): `siar-crash-recovery` 50/50, `siar-dtn-bundle` 56/56, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions, confirmed rather than assumed.

### Round 7 (2026-09-28) — §53-58

The filesystem-safety cluster: temp naming, path traversal, export sanitization, then MIME sniffing and executable-content detection — the six sections immediately after the storage/quota group (§47-52) this crate is deliberately still deferring (real filesystem staging/atomicity, not pure types).

- **§53 Temp File Naming**: new `filename_safety.rs` — `temp_file_name(transfer_id) -> String` (`"<uuid>.partial"`), built only from a locally-generated `TransferId`, never a remote-supplied name.
- **§54 Path Traversal Protection — resolved by omission, not a function**: `descriptor::FileName`'s own doc comment now states explicitly that there is no path-shaped API on it to misuse — the only sanctioned route from a `FileName` to real filesystem bytes is this round's own `sanitize_for_export`.
- **§55 Filename Sanitization**: `sanitize_for_export` (strips `/`, `\`, and control characters; falls back to a fixed safe name for empty/`.`/`..`) and `resolve_export_collision`, tested against the spec's own worked example verbatim (`photo.jpg` → `photo (1).jpg`).
- **§56 MIME Type Validation**: new `content_sniffing.rs` — `sniff_media_type`, a real magic-byte sniffer (PNG/JPEG/WebP/MP4/Opus-in-Ogg signatures) covering every `siar_domain::MediaType` variant that has one, and `check_declared_media_type` returning a real three-state `MimeTrust` (`Confirmed`/`Mismatch`/`Unverifiable`) rather than collapsing "couldn't check" and "checked and it's wrong" into one bool.
- **§57 Executable Content**: `has_potentially_executable_extension`, checking a `FileName`'s extension against a fixed risky-extension list (`.exe`, `.apk`, `.sh`, `.jar`, …) — `MediaType` itself has no executable-content variants to check, named honestly rather than faked.
- **§58 Archive Safety — no code anywhere in this crate**: the spec's own explicit instruction ("must be handled in a separate safe extraction subsystem. Blob layer itself stores opaque bytes").

**Verification**: 17 new tests, 96/96 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc` clean (no broken links this round). All five dependents re-tested in the same round: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 8 (2026-09-28) — §59-66

The blob-lifecycle cluster immediately after: reference counting, pinning, then the cache/durability classification and the GC/eviction/storage-pressure policies built on top of it.

- **§59 Deduplication — resolved by construction, no function needed**: `ids::BlobId::from_ciphertext` is already the *only* way to build a `BlobId`, so identical ciphertext always produces the identical ID — documented, not re-implemented.
- **§60 Reference Counting**: new `blob_references.rs` — `ReferenceKind` (the spec's own six named kinds) and `ReferenceSet`, a real multiset (`Vec`, not a `HashSet`) — two messages referencing the same blob are two references, both needing removal before `is_gc_eligible()` returns true.
- **§61 Pinning**: `PinReason`, the spec's own five named reasons — deliberately kept as a distinct vocabulary from `ReferenceKind` rather than merged with it (see that type's own doc comment: §60 asks "does this blob still need to exist," §61 asks the narrower "is this specific thing currently exempt from cache eviction").
- **§62 Cache vs Durable Blob**: new `storage_lifecycle.rs` — `BlobClassification` (`Durable`/`Cache`/`Temporary`/`Relay`).
- **§63 Garbage Collection, §64 Grace Period**: `is_gc_eligible(references, unreferenced_since, now, policy, has_active_transfer)` — structurally unable to base eligibility on age alone (the function never takes a creation timestamp at all, only how long a blob has been *unreferenced*), directly enforcing the spec's own "never delete a blob solely because it is old."
- **§65 Cache Eviction**: `CacheEvictionTier`, the same `Ord`-by-declaration-order pattern `siar_dtn_bundle::eviction::EvictionTier` already established in this workspace (see `docs/06`), plus `may_evict`'s explicit, named `emergency_override` parameter — a caller can't accidentally bypass eviction protection by a silent default.
- **§66 Storage Pressure**: `StoragePressure` (`Normal`/`Elevated`/`Critical`/`Full`) and `effects_for`, a deterministic escalating mapping across all four tiers (this crate's own reasonable reading of the spec's five named effects as an ordered escalation, since the spec gives the effects as a flat list rather than a per-tier table).

**Verification**: 15 new tests, 111/111 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc` clean. All five dependents re-tested in the same round: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 9 (2026-09-28) — §67-80

The largest single cluster yet (14 sections, at the user's own request to cover more ground per round): relay/DTN delivery policy, emergency file priority, multi-device and own-device sync targets, peer availability advertisement, source-selection vocabulary, and — the round's real find — a manifest-authentication gap.

- **§67 Relay Blob Storage**: new `relay_and_dtn.rs` — `RelayStorageAccounting` (quota/expiry), structurally incapable of carrying a decryption key ("relay node does not get content keys" enforced by there being no such field to add, not by a runtime check).
- **§68 DTN Chunk Strategy**: `DtnChunkPolicy` (`MetadataOnly`/`Thumbnail`/`SelectedChunks`/`FullFile`) and `chunks_to_send`, keeping "a subset of this blob's own chunks" and "a separate thumbnail blob with its own `BlobId`" as genuinely different cases — `Thumbnail` resolves to `None`, not an empty or wrong chunk list.
- **§69 Emergency Files**: new `delivery_policy.rs` — `EmergencyFileKind`/`EmergencyFilePriority` (`Ord`-ranked, same declaration-order-is-rank pattern as spec 06's `EvictionTier`) and `suggested_emergency_priority`, named honestly as this crate's own suggestion — "Part 17 decides priority," per the spec's own words, so `siar-emergency` owns the real enforcement.
- **§70 Multi-Device File Delivery**: `FileDeliveryTarget` (`Account`/`SpecificDevice`/`AllDevices`), using `siar_domain::{AccountId, DeviceId}` — the first time this crate has reached for those two types rather than staying purely local vocabulary, since resolving an account to its devices is explicitly Part 02's own job.
- **§71 Own-Device Blob Sync**: `OwnDeviceSyncPolicy`, with `FullFileToSelectedDevices` taking a real `Vec<DeviceId>` rather than reusing `FileDeliveryTarget::SpecificDevice` — the spec's own example ("full file to selected devices," plural) is one sync decision over a named set, not several independent single-device ones.
- **§72 Peer Discovery of Blob Availability**: `BlobAvailabilityAdvertisement` and `may_advertise_availability`, requiring both of the spec's own named preconditions (privacy policy AND authorized session) at once — satisfying only one is still a no.
- **§73 Swarming — no code**: the spec's own words, "not necessary for initial release."
- **§74 Source Selection**: new `source_trust.rs` — `BlobSource`, vocabulary only; actual selection stays `siar-routing-policy`'s job per the spec's own "routing chooses path/source according to policy."
- **§75 Trust of Sources — resolved by construction, no function needed**: documented as the reason `verify::verify_chunk`/`verify::verify_complete_blob` matter, not a new check.
- **§76/§77 — a real gap found by reading them together**: `verify::verify_complete_blob` (§76) only checks a manifest for internal self-consistency — does the reassembled ciphertext match `manifest.blob_id`? — never against an externally-authenticated expected `BlobId`. An attacker who swaps in an entirely different, internally-consistent manifest (its own `blob_id` matches its own reassembled ciphertext) would pass `verify_complete_blob` cleanly, which is exactly the reorder/replace attack §77 warns about. New `source_trust::verify_manifest_authenticity(manifest, expected_blob_id)` closes it — proven by a test that constructs exactly that attack (a real "attacker" manifest that passes `verify_complete_blob` on its own, then fails `verify_manifest_authenticity` against the real expected id) rather than only testing the happy path. `verify.rs`'s own doc comment updated to point at this.
- **§78 Merkle Tree Option — no code**: the spec's own words, "a flat chunk-hash manifest may be sufficient initially."
- **§79/§80 Compression — no code, deliberately**: recorded as a real, deferred decision rather than silently skipped — building either section means picking and adding an actual compression codec dependency, which this round doesn't do on a future caller's behalf. The required pipeline order (plaintext → optional compression → encryption → chunking) and the "never compress ciphertext" rule are named in `lib.rs` for whoever picks this up.

**Verification**: 10 new tests, 121/121 total in `siar-blob-manifest`. One real doc-link bug caught and fixed by `cargo doc` (referenced a `crate::eviction` module that doesn't exist in this crate — the eviction-tier pattern actually lives in `siar-dtn-bundle`; this crate's own equivalent is `storage_lifecycle::CacheEvictionTier` — corrected to point at the right type). `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. All five dependents re-tested in the same round: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 10 (2026-09-29) — §98-106, §118-119, §140-141

Transfer-control messages, source-mutation detection, and the progress/ETA model — 13 sections in one round.

- **§98 Transfer ACK, §99 Chunk ACK, §100 Completion ACK**: new `transfer_control.rs` — `TransferAckKind` (four genuinely distinct states, not one flag), `allows_transfer_completion` (true only for `BlobVerified`), and `ChunkAck`, reusing `ResumeBitmap`'s own range machinery — a new `ResumeBitmap::received_ranges` was added as the exact complement of the pre-existing `missing_ranges` (both now share one internal scan function, `ranges_where`, parameterized by which boolean counts as "inside a run," rather than two copies of the same loop that could drift apart).
- **§101 Transfer Cancellation**: `StagingRetentionPolicy` — the one real decision point among the section's four named actions (the other three — stop scheduling, cancel streams, persist `Cancelled` — are unconditional, so none gets a field).
- **§102 Pause — no new type**: already exactly what `transfer_journal::TransferJournal` holds (chunk bitmap, bytes verified, retry count) — documented as sharing the same retention mechanism §101 exposes, not a second one.
- **§103 Deferred Transfer**: `DeferredReason`, the spec's own five named reasons.
- **§104 Retry — already satisfied, no new work**: `transfer_planner::RetryPolicy` already operates at transfer/session level, never per packet.
- **§105 Corrupt Chunk**: `on_corrupt_chunk(prior_failure_count, penalize_threshold)` — real escalating state, not a bare discard-and-retry flag; source penalization triggers only once the running failure count crosses a caller-supplied threshold, not on the first corrupt chunk.
- **§106 Corrupt Manifest — resolved by construction**: this crate has never had a partial-reconstruction code path to disable; `build_manifest`/`verify_complete_blob`/`verify_manifest_authenticity` are already all-or-nothing.
- **§118 Source Mutation Detection**: new `source_integrity.rs` — `SourceSnapshot` (size/mtime/file-id/content-hash-at-read) and `source_mutated`, where any single field disagreeing is enough — deliberately not a majority-vote check.
- **§119 Immutable Source Snapshot — no type**: an I/O staging strategy this crate has no filesystem layer to represent; documented as the reason `SourceSnapshot` is a fallback for when that strategy isn't used, not a requirement.
- **§140 Progress Model**: new `progress.rs` — `TransferProgress`, transcribed field-for-field from the spec's own literal Rust snippet, plus a `percent_complete` helper (100% for a zero-byte transfer, not a `NaN`).
- **§141 ETA**: `EtaEstimate`/`estimate_eta` — a real, distinct `Unknown` variant (no active rate, a stalled zero rate, or nothing left to estimate) rather than a sentinel smuggled through the number, directly enforcing "do not present false precision."

**Verification**: 16 new tests, 137/137 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc` clean (no broken links this round). All five dependents re-tested in the same round: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 11 (2026-09-29) — §91, §96, §108, §110-113

Quarantine, a real internal-representation upgrade for `ResumeBitmap`, and rate-limit/resource/battery/memory/thermal policy.

- **§91 Quarantine**: new `quarantine.rs` — `QuarantineRecord`, deliberately kept separate from `storage_lifecycle::BlobClassification` — quarantine isn't a fifth lifecycle tier, it's outside that system entirely, which is what "do not expose to normal applications" actually means.
- **§96 Chunk Bitmap Optimization — this round's real structural improvement**: `ResumeBitmap` rewritten from `Vec<bool>` (one byte per chunk) to a hand-rolled bit-packed `Vec<u64>` (one bit per chunk — 64× less memory at the "thousands of chunks" scale the section names), with `is_complete`/`received_count` doing word-level bulk checks instead of a per-chunk scan. Every public method's signature stayed identical, so every existing caller (`transfer_planner`, `transfer_control`, `partial_availability`) kept working with zero changes — proven by the full 141-test suite passing unchanged, not just the four new bit-packing-specific edge-case tests (word-boundary marks, a non-multiple-of-64 chunk count, a zero-chunk bitmap, an out-of-range mark).
- **§108 Rate Limiting**: new `resource_policy.rs` — `RateLimitPolicy`, the same two-tier known/unknown-peer shape `siar-dtn-bundle`'s own `PeerQuotaPolicy` already established for storage quota (see `docs/06`), applied here to rate.
- **§110 Resource Policy**: `FileResourcePolicy`, the spec's own literal struct, transcribed field-for-field.
- **§111 Battery Awareness**: `BatteryAwareAdjustments`, named honestly as a minimal stand-in — Part 13 (Battery-Aware Scheduling) doesn't exist yet anywhere in this workspace (it's the single highest-priority spec after Tier 0 per `ROADMAP.md`'s own cross-spec ranking).
- **§112 Memory Pressure**: `MemoryPressureAdjustments` — "do not lose verified progress" enforced by omission: none of its four actions touch `TransferJournal::bytes_verified`.
- **§113 Thermal Pressure**: `ThermalPressureAdjustments`.

This round also flagged §92-95/§97/§107/§109/§114 as resolvable without new code — the user pushed back on that call afterward, so round 12 revisits all five with real implementations instead (see below). Nothing from this paragraph should be read as the final word on those five sections anymore.

**Verification**: 6 new tests plus 4 new `ResumeBitmap` edge-case tests, 143/143 total in `siar-blob-manifest`. One real doc-link bug caught and fixed by `cargo doc` (the same `partial_availability` function/module ambiguity from an earlier round, reintroduced by this round's own doc additions — corrected with `mod@`). `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. All five dependents re-tested in the same round, with particular attention paid since `ResumeBitmap`'s internals changed underneath every caller: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 12 (2026-09-29) — §92-95, §97, §107, §109, §114: brought in for real

The user explicitly rejected round 11's "no code needed" call on these five sections and asked for them to be genuinely implemented. Re-reading each with that instruction found real, legitimate code in every case — this round is the correction.

- **§92 Filesystem Permissions**: new `schema_rows.rs` — `FilesystemPermissionPolicy`, a real owner-only-mode-bits value (`0o700` directories, `0o600` files). Actually applying it via `chmod` is still real OS I/O this crate doesn't perform, but the restrictive numbers themselves are no longer just prose — a future caller has an actual value to apply.
- **§93-95 Database Schema/Blob Table/Transfer Chunk Table**: `schema_rows.rs` also adds real row-shape types for five of the spec's eight named conceptual tables — `BlobRow` (§94, field-for-field, with no path field of any kind: "do not store content path from untrusted metadata" enforced by there being nowhere to put one), `BlobDerivative`/`DerivativeKind` (generalizing §68's thumbnail concept into a real link between a derived blob and its source), `TransferChunkRow` (§95, field-for-field), `TransferSourceRow` (binding a `BlobSource` kind to one specific transfer — `BlobSource` alone is just vocabulary, never tied to a transfer until a row says so), `BlobPinRow` (same binding relationship, for `PinReason`). The other three conceptual tables already had a real type elsewhere in this crate (`blob_references` → `ReferenceSet`, `transfers` → `TransferRecord`, `storage_quotas` → `FileResourcePolicy`) — the module's own doc comment maps all eight explicitly. A new `ReferenceSet::len()` (plus `is_empty()`) was added so `BlobRow::reference_count` has a real number to report, not a placeholder.
- **§97 Durable Progress**: `TransferProgress::from_journal` — the one sanctioned constructor, pulling `verified_bytes` directly from `TransferJournal::bytes_verified` rather than leaving "derive from verified bytes, not bytes received into memory" as a claim about a field name alone. A test proves a retry (which never touches `bytes_verified`) can't inflate the reported progress.
- **§107 Multiple Sources**: new `transfer_control::SourceFailureTracker` — a real per-`DeviceId` failure counter built on `on_corrupt_chunk`, where "preserving other sources" is enforced by the `HashMap` keying itself (proven by a test with one bad source and one good source, checking the good one is never blacklisted), not left as caller convention.
- **§109 Abuse Protection**: new `tests/abuse_protection_matrix.rs` — a consolidated integration test, one per named threat (huge fake size, millions of chunks, never-completing transfers, hash-mismatch spam, storage exhaustion, offer spam), each exercised against the real existing mitigation rather than only mapped in prose. If a future change silently weakens any one of the six, exactly one of these six tests fails — not a unit test buried in an unrelated module.
- **§114 Hashing Performance**: new `BlobId::from_ciphertext_incremental`, a genuinely usable entry point built on blake3's own streaming `Hasher` that a future streaming reader can call today, feeding it chunks as they arrive rather than buffering a whole ciphertext first. Three new tests prove it produces the IDENTICAL `BlobId` as whole-buffer `from_ciphertext` — split into uneven pieces, a single chunk, and zero chunks — §114's own real correctness requirement, checked directly rather than assumed.

**Verification**: 11 new unit tests plus 6 new integration tests, 154 unit + 6 integration = 160/160 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean (including the new integration test file). `cargo fmt --check` clean. `cargo doc` clean, no broken links. All five dependents re-tested in the same round: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

### Round 13 — Local Blob Store (§37, §39, §40, §47-52, §86-90, §186-188, §38 partly)

Added the local blob store subsystem:
- **§37 Streaming I/O / §39 Low-Copy Data Path / §40 Buffer Pools**: zero/low-copy streaming primitives and buffer management for blob staging.
- **§47-52 Local Storage & Staging**: staging directory, chunk validation, atomic finalization into durable content-addressed storage, quota checks, and crash resilience during write.
- **§86-90 Storage Management**: disk layout, content addressing verification, storage metrics.
- **§186-188 Integration**: integration points for the storage layer.

### Round 14 — §148-157, §154, §79-81, §82-85: Reference Store, GC, Compression & Transfer Engine

- **§148-157 Reference store & GC**: reference store implementation, GC eligibility evaluation, and staging retention (+ §150 recount).
- **§154 Typed expiries**: explicit expiration policies for cached and temporary blobs.
- **§79-81 Compression**: pure-Rust DEFLATE/flate2 codec decision and tested zip-bomb / compression bomb defenses.
- **§82-85 Media derivatives**: derivatives, voice notes, and channel separation.
- **Wired into `siar-file-transfer-service`**: `transfer_engine` supporting receive, pause, resume, cancel, restart, late-completion, and `restore_from_log`.
- **Status at round close**: ~38 sections remaining, mainly §141-147/§158-185 (acceptance UX, backup, capability/forwarding/re-keying, multipath, fairness, diagnostics), §190-209 (test layers, public API, definition of done), §41-43 parallelism, §83 thumbnail *generation* (needs an image codec). Send-side engine and GC scheduling not yet wired.

### Round 15 — §132-139, §174-185: Wire Framing, Scheduling & Protocol Wiring

- **§132-139 `wire`**: `files/1` control frames, bounded manifest frame, fixed 88-byte chunk header validated before allocation, capability negotiation (§133-134).
- **§174-185 `scheduling`**: path-change plans, weighted fair scheduler with per-peer/per-app caps, metrics, diagnostics.
- **Wired**: `siar-file-transfer-service::protocol` (`FileReceiver`/`FileSender`, end-to-end test over real stores incl. pause/resume-missing and forged chunks) and `gc_schedule::GcScheduler` (`gc::sweep` now has a caller; `TransferEngine::gc_sweep` protects in-flight blobs).
- **Verification**: 261 unit + 8 protocol tests, clippy `-D warnings` and fmt clean. ~25 more sections addressed; §130/131 deliberately not built (no iroh-blobs backend exists).

### Round 16 (§201-205, new crate `siar-files`) — Public API & Messaging Integration

- **`FilesService` as Public API**: `send_file(Destination, source, FileSendPolicy)` → `SendHandle{pause,resume,cancel}`, `next_offer()` → `IncomingOffer{accept,reject}`, `import_attachment`/`bind_to_object`/`open_attachment`, `FileSource` (bytes, path; length checked before allocation, source-mutation detected), driven by one `handle_frame` entry point over a host-supplied `FileChannel`.
- **`siar-messaging` integration**: `send_imported_attachment` (shared `send_attachment_reference` with the old path) and `FilesBlobStore` (serves the file store to peers; no second copy in siar-storage).
- **Verification**: Verified over two real iroh endpoints: imported → sent → recipient's UNCHANGED `fetch_attachment` decrypts it (the two attachment formats are byte-compatible).

### Round 17 — FairScheduler & GC Maintenance

- **`FairScheduler` integration**: `FilesService` now serves chunks through `FairScheduler` (class weights, strict emergency priority, round-robin across peers/apps; replaces the per-request task).
- **GC Ownership**: `maintenance(now)` / `spawn_maintenance(period)` drive `GcScheduler`.
- **Verification**: 14 `siar-files` tests including a multi-transfer fairness run and a grace-period GC run.

### Round 18 (§115-117, §120-129) — File I/O, Plaintext Cache, Preprocessing & Streaming AEAD Finding

- **`file_io`**: async `FileSource`/`FileSink`, bounded read that refuses a source that grows or changes, fs source opened once, fs sink atomic/no-overwrite/cleans up.
- **`plaintext_cache`**: off by default, byte+age bounded, LRU, zeroized.
- **`preprocess`**: fixed §115 order, §116-117 staging decision, `Verified<T>` for §129.
- **`siar-files` additions**: `export_attachment` (§125-126; an auth failure writes nothing), `open_verified_attachment`, optional plaintext cache.
- **Design finding: whole-blob AEAD blocks streaming**: Stream-only send (§117) and memory-free export need a chunk-wise AEAD blob version (e.g. per-chunk nonce from index, tag per chunk); `STREAMING_ENCRYPTION_SUPPORTED=false` makes `StreamOnly` refuse until then — needs a format decision.
- **Verification**: 25 new blob-manifest tests (286 total), 13 siar-files tests; clippy/fmt clean; messaging suites unchanged.

### Round 19 (chunked AEAD v2 + §142-147, §158-173) — Chunked AEAD v2, App Policies & Key Wrapping

- **Blob format v2 (`chunked_encryption`)**: per-segment auth, index-in-nonce, last-flag, authenticated header; single-bit-flip exhaustive test.
- **`ManifestBuilder`**: incremental builder, equals `build_manifest`.
- **`siar-files` streaming transfers**: `import_attachment_streaming`, `send_file_streaming`, segment-by-segment `export_attachment` (memory bounded by one segment; verified: largest read ≤ 256 KiB for a 3 MB file; corruption in a late segment leaves NO file at the destination).
- **Policies & features**: `app_policy` (notifications, acceptance prompt, auto-download, data saver, storage classes), `backup`, `sharing`, `key_wrapping`. `FilesService` takes a `NotificationSink` and optional auto-download policy (Wi-Fi images accepted unasked, metered deferred to the app) and exposes `IncomingOffer::prompt()`.
- **Verification**: 328 blob-manifest + 22 siar-files tests; clippy/fmt clean; messaging suites unchanged.
- **Deferred / not built**: true stream-to-network driver with no staging (the format now allows it; `decide_staging` says so; no driver); v2 is opt-in because a v1-only peer cannot decrypt it and the attachment reference carries no version; multi-recipient wrapping not yet connected to a real send (needs the group/session secret source); data saver and storage classes are policy values no scheduler/evictor consumes yet.

### Round 20 (§41-43, §191-196) — Parallelism, AIMD Adaptive Mode & Proptests

- **`parallelism`**: per-path ceilings, resource reductions, AIMD adaptive mode inside the deterministic limit.
- **Pipelined pump**: `FilesService` pump now keeps N chunk sends genuinely in flight per peer (`set_path_kind`, `set_resources`, `report_sample`; measured LAN > 1, Bluetooth == 1, battery-saver ≤ 2), per-peer caps in the scheduler.
- **Property testing**: 11 proptest properties (decoder robustness on hostile bytes, bitmap partition, any-order/duplicate reassembly, corruption detection, v2 mutation rejection, small-file overhead, GC never deletes protected blobs).
- **Large-file test**: 32 MiB large-file bounded-memory test.
- **Verification**: 338 blob-manifest + 25 siar-files tests; clippy/fmt clean; messaging suites unchanged.

### Round 21 (§189-198 gaps) — Crash Injection, Hostile Network, Scenarios & Fuzzing

- **(1) §193 Crash injection** (`siar-file-transfer-service/tests/crash_injection.rs`): crash after EVERY possible chunk count (0..=n, incl. all-staged-not-finalized) recovers byte-exact, referenced once, no staging left; journal-behind-disk; torn staging (flipped byte / cut mid-chunk / zeroed page) resumes from exactly the chunks that re-verify; published-but-damaged blob is never reported complete; GC crash states (file deleted/row kept, row deleted/file kept) are repaired by `reconcile_metadata` with no referenced blob lost; GC is idempotent. Service-level: receiver dies after offer (nothing staged/referenced, sender cancels cleanly); lost completion ACK (sender keeps holding blob; re-offer costs zero chunk frames via dedup).
- **(2) §190 Hostile network**: test channel loses (30%), duplicates (20%) and reorders (≤25 ms jitter) chunk frames deterministically; 6 MB transfer arrives byte-exact (asserts frames were really lost, duplicated, and retransmitted). Exposed and built missing feature: `FilesService::retry_stalled` (+ `FileReceiver::request_missing`, run by `spawn_maintenance`) re-requests missing ranges or manifest for transfers with no progress; lost manifest request retried to completion. Also added `send_imported` (re-offer an already-imported blob under its own reference).
- **(3) §197 Bluetooth scenario**: cut off mid-transfer on BLE (exactly one chunk in flight), upgraded to Wi-Fi Direct, resumes with only missing chunks (frames ≤ chunks+2) and overlaps afterwards.
- **(4) §198 DTN/emergency scenario**: policy-layer only (`tests/scenarios.rs`): thumbnail first / original deferred, small critical → DTN, bulk held, DTN parallelism 1, resume of exactly the missing ranges (the DTN transport itself `siar-dtn-bundle` is not exercised here).
- **(5) §192 Coverage-guided fuzzing**: 5 cargo-fuzz targets in `crates/siar-blob-manifest/fuzz` (control/manifest decoders, chunk frame, v2 decrypt, builder≡build_manifest); compile on stable (`cargo check`), libFuzzer execution requires `cargo +nightly fuzz run`.
- **Verification**: 338 + 11 properties + 2 scenarios (blob-manifest), 6 crash-injection (transfer service), 30 siar-files e2e; clippy/fmt clean; messaging suites and cli unchanged.

### Round 22 (durable-store crash injection) — Stoolap Persistence & Paging Fix

- **Durable crash protocol** (`siar-file-transfer-service/tests/crash_durable.rs`): runs crash protocol with blob metadata, reference model, and event log all on stoolap files, every handle dropped and reopened from disk: crash after every chunk count 0..=n (durable reference survives on its own BEFORE recovery runs, resume never doubles it), durable log replays to pre-crash state, completed transfer survives restart, repeat offer dedups, cancel-after-crash releases durable reference (and stays released across another restart), and GC crash states (row-without-file, file-without-row) are REAL persisted states repaired by `reconcile_metadata`, with repair itself durable.
- **Production bug found and fixed**: `reconcile_metadata` called `meta.list(usize::MAX)`, and stoolap-backed stores passed `LIMIT i64::MAX` straight to the engine, whose top-N operator tries to reserve that many rows and panics (`capacity overflow`) — so GC repair after a crash would have aborted on the durable store. Fixed in `StoolapBlobMetadata` (`list`/`list_after` page internally in 1024-row queries and honor limits), `StoolapReferenceStore` (`referenced_blob_ids` pages; `references_for` capped at 16,384 per query), and `reconcile_metadata` (cursor-paged instead of materializing every row).
- **Regression test**: `tests/durable_paging.rs` (2,600 rows across page boundaries, every limit around 1024/2048, odd cursor page sizes, both durable stores).
- **Verification**: 338 + 4 paging + 11 properties + 2 scenarios (blob-manifest), 5 durable + 6 in-memory crash-injection (transfer service), 30 siar-files e2e; clippy/fmt clean; messaging suites and cli unchanged.

### Round 23 (files/1 over real QUIC, and pull) — Network ALPN Handler & PULL

- **(1) `siar-transport` `files/1` protocol**: `FILES_ALPN`, `FilesProtocolHandler`, `SiarEndpoint::bind_with_files` / `send_files_frame`, `DeferredFilesSink` to break endpoint↔service construction cycle. Frames are `[kind u8][len u32][bytes]` on ONE persistent unidirectional stream per peer, written under lock because `files/1` needs frames in order and separate QUIC streams are not ordered; lengths checked before allocation (`try_reserve_exact`); failed stream replaced once.
- **(2) `siar-messaging` integration**: `PeerBook` (device↔endpoint, unpaired endpoints' frames dropped), `TransportFileChannel`, `FilesInbound`, and `fetch_attachment_over_files`.
- **(3) PULL transfers**: closed the gap where `fetch_attachment` still pulled whole blobs: two appended control messages (`BlobRequest`, `BlobUnavailable`; appended so older peers refuse them cleanly), `FilesService::fetch_blob` / `fetch_attachment`, and `ServeGate` (default: serve only still-referenced blobs; refusal, absence and unreadable all look identical to asker). Holder rebuilds descriptor+manifest by streaming stored blob once (format sniffed from bytes), offers it under own send reference, and requester auto-accepts offer for exactly the blob asked for; manifest authenticity and per-chunk/whole-blob hashing apply so wrong blob cannot land under requested id. Concurrent fetches share one request; failing redundant transfer does not fail working fetch; timeouts and local hits handled.
- **Verification**: Verified over TWO REAL iroh endpoints: push, v2 multi-chunk push, simultaneous bidirectional transfers, reject frees sender, unpaired peer dropped, unknown address fails clean and leaves no reference, pull-by-reference with decrypt, pull-unavailable. Counts: 338 + 4 + 11 + 2 (blob-manifest), 11 (transport incl. 5 files-handler), 39 siar-files e2e, 8 files-over-iroh, messaging suites; clippy `-D warnings` and fmt clean.

### Round 24 (the Round-23 "not done" list) — File Key/Name Carriage & First-Response Deadline

- **Provenance note**: working tree already contained sender-side offer re-send (`retry_unanswered_offers`), manifest cache, and `AttachmentFetcher`/`FilesAttachmentFetcher` fallback in `MessageService::fetch_attachment` with tests.
- **(1) File key + name carriage**: `send_file` encrypted under a fresh key and previously never delivered it, so a file-only receiver could verify ciphertext but never open it. Offers now carry key and optional display name (`offer_meta`, bounded, parsed defensively — garbage means "no metadata", never an error); receiver gets `IncomingOffer::display_name()` and on completion `ReceivedFile` (`next_received()`, an UNBOUNDED queue because it holds the only copy of the key) with `reference()`, `display_name()`, and `export_name()` (sanitized: no separators, never empty/`..`); pull carries no metadata (requester already has reference).
- **Security note (documented in `offer_meta`)**: key rides inside offer, which is as private as the authenticated end-to-end QUIC connection it travels on — it must NOT be used over a relay or DTN custodian; use `key_wrapping` per recipient there.
- **(2) First-response deadline**: `fetch_first_response_millis` (default 8 s). Peer that never reacts (no offer, no refusal) now fails with `FilesError::NoResponse` quickly, mapped to `Unsupported` so legacy blob path runs in seconds rather than after 60 s overall timeout; slow responder still gets full timeout (`FetchTimeout`).
- **(3) Timing bug fix**: fixed timing bug in resend test (waited 250 ms before asserting nothing re-sent against 100 ms stall window).
- **(4) Clippy hygiene**: offer parameters bundled into `BlobToSend` instead of 8-argument function. Explicit exit codes recorded for verification.
- **Verification**: 586 tests pass across blob-manifest, transfer-service, siar-files, siar-transport, siar-messaging (siar-files: 59 e2e + 5 unit); clippy `-D warnings` clean.

### Round 25 (wrapped-key and multi-recipient delivery) — Pairwise Secrets & Multi-Recipient Send

- **(1) `siar-crypto` keywrap domain**: new domain `comm/file-keywrap/v1` and `FileKeyWrapSecret::establish(identity, peer_x25519_public)` — pairwise secret wrapping blob keys, derived from same X25519 exchange as session but with independent domain label (tested independent of session and mailbox secrets; same static-key forward-secrecy limit as `Session`).
- **(2) Strict 60-byte encoding**: `key_wrapping::WrappedKey::{to_bytes, from_bytes}`.
- **(3) `siar-files` multi-recipient send**: offer metadata v2 carries key wrapped for ONE recipient (raw key does not appear in bytes); `KeyDelivery::{InOffer, Wrapped}` per send; `PairwiseSecrets` trait (host supplies secrets; crate never sees long-term keys); `IncomingOffer::key_status()` (`Present` / `WrapUnopenable` / `Absent` — receiver with no/wrong secret still gets verified bytes and display name, never key and never `ReceivedFile`); `send_file_to_many`: ONE import, ONE stored blob, one wrapped key per recipient; requires `Wrapped` (raw key never sent to multiple parties); recipients fail independently; each transfer holds own reference and import reference released when offers out.
- **(4) `siar-messaging` integration**: `PeerBook::register_with_key`, `IdentityPairwiseSecrets` (real identity + recorded peer key; peer with no recorded key has NO secret — no wrap made against a guess).
- **Verification**: 3-device star topology (one blob, both offers name same BlobId, both recipients open it, sender transfer references end at 0); over REAL QUIC with REAL identities: wrapped file opens on other device, key wrapped against wrong public key does not, peer with no recorded key gets no wrap. 671 tests passed, 0 failed; clippy `-D warnings` clean.

### Round 26 (streaming multi-recipient) — Streaming Multi-Recipient Send

- **`send_file_to_many_streaming`**: one-blob / one-wrap-per-recipient fan-out with v2 chunk-wise format and bounded memory.
- **Shared fan-out helper**: fan-out loop is a shared private helper between streaming and non-streaming multi-recipient entry points to prevent drift.
- **Verification**: 3-device star verified: 2.8 MB source never read in pieces larger than 256 KiB, both offers name SAME v2 blob, both recipients open it, sender transfer references end at 0; wrapped-keys-only rule enforced on streaming path; recipient with missing secret fails without stopping others; empty recipient list leaves nothing referenced. 70 e2e + 7 unit tests in siar-files, 678+ tests workspace-wide; clippy `-D warnings` clean.

### Round 27 (revocation, cache pruning — and deadlock fix) — Access Revocation & Cache Pruning

- **(1) Revocation as a service operation**: `FilesService::revoke_access(blob, recipient)` / `restore_access`, backed by `AccessLedger` trait (default `MemoryAccessLedger`; durable implementations supported). Revoking stops FUTURE access only: no further key sent (`FilesError::AccessRevoked`), pulls from that device refused as "not here", unfinished transfers cancelled. Cannot recall keys or ciphertext already delivered (`StopsFutureAccessOnly`, `recipient_may_hold_key` — verified by test where recipient who already had file can still open it). Revoking one of two recipients leaves the other untouched.
- **(2) Manifest LRU cache**: upgraded to real LRU cache with hit-recency refresh (preventing FIFO eviction of hot manifests after 16 unrelated inserts); `maintenance()` prunes entries for blobs GC reclaimed (`prune_manifest_cache`).
- **(3) Bug found and fixed — self-deadlock in cancel handler**: when a sender cancelled a transfer the receiver had ALREADY ACCEPTED, the receiver's inbound handler hung forever: `if let Some(b) = self.incoming_blob.lock().remove(&id) { self.fail_fetch_unless_another_transfer(b) }` kept the scrutinee mutex guard alive for the whole block while the helper locked `incoming_blob` again. Fixed both call sites by binding the removed value first. Red/green verified: hangs without fix (exit 124), passes in 0.5 s with fix. Regression tests: `cancelling_an_accepted_transfer_leaves_the_receiver_fully_working` and `a_holder_cancelling_a_pull_midway_fails_the_waiting_fetch_promptly`.
- **Verification**: clippy `-D warnings` exit 0 over `siar-crypto`, `blob-manifest`, `transfer-service`, `siar-files`, `siar-transport`, `siar-messaging`; 693 tests passed, 0 failed; cli and dependents compile.

## Known gaps / open questions

### Immediate & High-Priority
- **Wrapped-key revocation at service level**: `key_wrapping::revoke` exists in `siar-blob-manifest`, but nothing calls it at the service level (and it can only stop future wraps, as documented).
- **Multi-recipient deduplication**: `send_file_to_many` / `send_file_to_many_streaming` does not de-duplicate against a recipient that already holds the blob.
- **Offer re-send on sender side**: re-sending unanswered offers is currently handled as an in-session concern.
- **Pull manifest caching**: the manifest rebuilt for a pull is streamed from disk on every request without caching.
- **Offer metadata encryption**: offer metadata carries display name in plaintext inside `offer_meta`; encrypted metadata carriage is not yet in offers.

### Integration & System Wiring
- **`MessageService::fetch_attachment` fallback**: still uses legacy `BLOB_ALPN` path by default; apps switch by calling `fetch_attachment_over_files` (both paths maintained for backwards compatibility with non-files/1 peers).
- **Periodic maintenance caller**: `spawn_maintenance` exists in `FilesService`, but no top-level application daemon invokes it yet.
- **Fetch attachment engine integration**: receive side of `fetch_attachment` in `MessageService` still pulls whole blob through `siar-transport` rather than `files/1` chunk streams (the engine path is verified in `siar-files` e2e tests, not yet wired to app transport).
- **`MAX_ATTACHMENT_BYTES` vs `ManifestLimits`**: `siar-domain` caps attachments at 200 MiB, whereas `ManifestLimits` allows up to 10 GiB; larger files are refused by domain limits.
- **True stream-to-network driver**: format v2 permits zero-staging streaming, and `decide_staging` supports it, but an end-to-end direct-to-network driver without intermediate disk staging is not yet implemented.
- **Audio/Image thumbnail generation**: §83 thumbnail generation requires an image/audio codec.
- **DTN transport integration**: DTN file policy scenarios (§198) are tested at policy layer; integration with `siar-dtn-bundle` transport wire format remains.
- **Coverage-guided fuzzing on nightly**: 5 cargo-fuzz targets compile on stable (`cargo check`), but running libFuzzer requires `cargo +nightly fuzz run`.
- **Pagination & scale**: `references_for` caps at 16,384 rows per query (documented constraint).

### Historical Open Items
- §17 Manifest Hierarchy — explicitly "not required initially" per spec; not attempted.
- §33-34 Progressive images/video — format and progressive decode considerations.
- §22's two named fields with no home yet in `FileMetadata` (dimensions, duration).

## Note

Updated 2026-10-26 (round 27). The subsystem spans `siar-blob-manifest`, `siar-file-transfer-service`, `siar-files`, `siar-transport`, and `siar-messaging`. 693 tests pass across crates with clean clippy `-D warnings` and fmt. Round-by-round history was consolidated here from `ROADMAP.md` to keep the roadmap concise and maintain this spec as the primary detailed reference.
