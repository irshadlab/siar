# Part 05 — Robust File / Blob Subsystem Architecture

Source spec: `sys-arch/05-robust-file-blob-subsystem-architecture.md` — 210 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~93/210 (44%) sections.** Partial — approximate, not from a full section-by-section audit like spec 04's (see that file for what such an audit looks like). §20/§21/§36/§54/§59/§73/§75/§78/§92-95/§97/§104/§106/§107/§109/§114 counted as resolved or explicitly deferred rather than open (see below), not as new code.

## Implementing crate(s)

- `siar-blob-manifest` — sole implementing crate.
- Cross-references: `events.rs` here implements `04-offline-event-log-architecture.md` §34 File Events, not one of this spec's own 210 sections — tracked in `docs/04`, not here. `siar-crash-recovery`, `siar-dtn-bundle`, `siar-emergency`, `siar-event-registry`, and `siar-file-transfer-service` all depend on this crate.

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

Quarantine, a real internal-representation upgrade for `ResumeBitmap`, and rate-limit/resource/battery/memory/thermal policy, plus five sections resolved without new code.

- **§91 Quarantine**: new `quarantine.rs` — `QuarantineRecord`, deliberately kept separate from `storage_lifecycle::BlobClassification` — quarantine isn't a fifth lifecycle tier, it's outside that system entirely, which is what "do not expose to normal applications" actually means.
- **§92-95 Filesystem Permissions/Database Schema/Blob Table/Transfer Chunk Table — no code**: OS-level and SQL-schema concerns for a real store this crate doesn't have; premature to design a schema for a backend that isn't built yet.
- **§96 Chunk Bitmap Optimization — this round's real structural improvement**: `ResumeBitmap` rewritten from `Vec<bool>` (one byte per chunk) to a hand-rolled bit-packed `Vec<u64>` (one bit per chunk — 64× less memory at the "thousands of chunks" scale the section names), with `is_complete`/`received_count` doing word-level bulk checks instead of a per-chunk scan. Every public method's signature stayed identical, so every existing caller (`transfer_planner`, `transfer_control`, `partial_availability`) kept working with zero changes — proven by the full 141-test suite passing unchanged, not just the four new bit-packing-specific edge-case tests (word-boundary marks, a non-multiple-of-64 chunk count, a zero-chunk bitmap, an out-of-range mark).
- **§97 Durable Progress — already satisfied, no new work**: `progress::TransferProgress::verified_bytes` (built last round) was already "verified bytes," never "bytes received into memory."
- **§107 Multiple Sources — no new type**: the multi-source generalization of §105's own `on_corrupt_chunk` — a caller tracks one failure counter per source; "preserving other sources" falls out for free from those counters being separate, not shared.
- **§108 Rate Limiting**: new `resource_policy.rs` — `RateLimitPolicy`, the same two-tier known/unknown-peer shape `siar-dtn-bundle`'s own `PeerQuotaPolicy` already established for storage quota (see `docs/06`), applied here to rate.
- **§109 Abuse Protection — no new type**: all six named threats already have a real mitigation elsewhere in this crate (`resource_policy.rs`'s own doc comment maps each one explicitly) — a seventh type here would only rename them.
- **§110 Resource Policy**: `FileResourcePolicy`, the spec's own literal struct, transcribed field-for-field.
- **§111 Battery Awareness**: `BatteryAwareAdjustments`, named honestly as a minimal stand-in — Part 13 (Battery-Aware Scheduling) doesn't exist yet anywhere in this workspace (it's the single highest-priority spec after Tier 0 per `ROADMAP.md`'s own cross-spec ranking).
- **§112 Memory Pressure**: `MemoryPressureAdjustments` — "do not lose verified progress" enforced by omission: none of its four actions touch `TransferJournal::bytes_verified`.
- **§113 Thermal Pressure**: `ThermalPressureAdjustments`.
- **§114 Hashing Performance — deferred alongside §37-39, no new work**: real incremental streaming hash needs streaming I/O this crate has already deferred; the section's own correctness requirement is already satisfied structurally — `BlobId::from_ciphertext` hashes whatever bytes it's given regardless of whether they arrived streamed or all at once.

**Verification**: 6 new tests plus 4 new `ResumeBitmap` edge-case tests, 143/143 total in `siar-blob-manifest`. One real doc-link bug caught and fixed by `cargo doc` (the same `partial_availability` function/module ambiguity from an earlier round, reintroduced by this round's own doc additions — corrected with `mod@`). `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. All five dependents re-tested in the same round, with particular attention paid since `ResumeBitmap`'s internals changed underneath every caller: `siar-crash-recovery` 50/50, `siar-dtn-bundle` 86/86, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-file-transfer-service` 6/6 — zero regressions.

## Known gaps / open questions

- §17 Manifest Hierarchy — explicitly "not required initially" per the spec's own text; not attempted.
- §33-34 Progressive images/video, §37-39 streaming/low-copy I/O and buffer pools, §41-43 parallelism/adaptive concurrency, §47-52 quotas/sparse files/staging/atomic finalization/crash recovery, §79-80 compression (pending a real codec dependency decision), §115-117/§120-139/§142+ except where named above — still a slice of a 210-section document, though a shrinking one.
- §22's own two named fields with no home yet in `FileMetadata` (dimensions, duration).
- No blob store exists in this workspace at all — everything here is pure types/decisions; `partial_availability`, `verify`, `transfer_planner`, `content_sniffing`, `storage_lifecycle`, `relay_and_dtn`, `delivery_policy`, `source_trust`, `transfer_control`, `source_integrity`, `progress`, `quarantine`, `resource_policy`, etc. are the computations a future store/engine would call, not a store or engine. Nothing yet calls `is_gc_eligible`/`may_evict`/`effects_for`/`verify_manifest_authenticity`/`estimate_eta` from a real running caller — the policy exists and is tested in isolation, not wired to a live loop.

## Note

Updated 2026-09-29 (round 11), transcribing §91/§96/§108/§110-113. Fraction is an approximation (`~93/210`), not a full section-by-section audit — unlike spec 04, this crate hasn't yet had a dedicated tracker-building pass.
