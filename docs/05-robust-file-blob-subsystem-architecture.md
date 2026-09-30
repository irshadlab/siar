# Part 05 — Robust File / Blob Subsystem Architecture

Source spec: `sys-arch/05-robust-file-blob-subsystem-architecture.md` — 210 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~83/210 (40%) sections.** Partial — expanded significantly in Rounds 6–11. 143/143 tests passing in `siar-blob-manifest`. Zero regressions across all 5 dependent crates.

## Implementing crate(s)

- `siar-blob-manifest` — sole implementing crate.
- Cross-references: `events.rs` here implements `04-offline-event-log-architecture.md` §34 File Events, not one of this spec's own 210 sections — tracked in `docs/04`, not here. `siar-crash-recovery`, `siar-dtn-bundle`, `siar-emergency`, `siar-event-registry`, and `siar-file-transfer-service` all depend on this crate.

## Round-by-round history

1. **Baseline (pre-2026-09-01)**: §5-8 (ids: `BlobId`, `ManifestId`, `ChunkHash`, `LogicalAttachmentId`), §9-10 (`descriptor.rs`: `BlobDescriptor`, `FileMetadata`), §11-13 (`chunking.rs`: fixed-size chunking), §14 (`verify.rs`), §15-16 (`manifest.rs`: `BlobManifest`/`build_manifest`), §18-19 (`encryption.rs`: whole-blob AEAD), §26 (`transfer_state.rs`: `TransferState`/`TransferEvent`), §29-30 (`resume.rs`: `ResumeBitmap`). Plus `metadata_encryption.rs`, which is actually Part 28 §31, not one of this spec's own sections — a real cross-spec module living here because the type it encrypts (`FileMetadata`) does.
2. **Phase 3 round 2, 2026-09-19 (see `docs/04` round 3 for full detail)**: §34 File Events, `04-offline-event-log-architecture.md`'s own section, not this spec's — `events.rs`, `EventTypeId` 200-208, `FileEvent` enum lined up with `TransferState`/`TransferEvent`. New `TransferId` newtype added to `ids.rs`. 37/37 tests at the time.
3. **§9 schema-version fix, 2026-09-19 (see `docs/04` round 7)**: `decode_file_event` gained real `schema_version` checking (`FileEventDecodeError::UnsupportedVersion`/`Malformed`), `CURRENT_FILE_EVENT_SCHEMA_VERSION` constant. 38/38 tests.
4. **§56 Durability Classes, 2026-09-25 (see `docs/04`)**: per-`FileEvent`-variant `durability_class()` classification added, part of a cross-domain pass touching all five event catalogs.
5. **§22-25/§27-28/§31-32 cluster, 2026-09-26**: §22 `metadata_privacy.rs`, §23-24 `attachment_reference.rs`, §25 `TransferId`, §27 `transfer_record.rs`, §28 `transfer_journal.rs`, §31 `ResumeRequest`, §32 `partial_availability.rs`. 60/60 tests at the time.
6. **Rounds 6–9 (Core Transfer & Lifecycle Policies, 2026-09-29)**:
   - **§35 Transfer Planner**: `transfer_planner.rs` — `TransferPlan`, chunk scheduling, missing-chunks-first ordering, concurrency, `RouteRequirements`, and exponential backoff retry planning. §36 "Transport Independence" honored by having zero transport dependencies.
   - **§44 File Offer Protocol**: `file_offer.rs` — `FileOffer` and receiver's `FileOfferDecision` (`Accept`, `Reject`, `Defer`, `MetadataOnly`).
   - **§45 Auto-Accept Policy**: `auto_accept.rs` — `AutoAcceptPolicy` and pure evaluation function `evaluate_auto_accept`.
   - **§46 Transfer Authorization**: `transfer_authorization.rs` — `authorize_transfer` enforcing peer authorization, size policy, storage quota, and content policy in order.
   - **§53 Temp File Naming / §54 Path Traversal / §55 Filename Sanitization**: `filename_safety.rs` — `temp_file_name` keyed by `TransferId`, `sanitize_for_export`, `resolve_export_collision` handling numbered suffix deduplication.
   - **§56 MIME Validation / §57 Executable Content**: `content_sniffing.rs` — `sniff_media_type`, `check_declared_media_type`, `has_potentially_executable_extension`. §58 archive safety deferred to separate sandbox.
   - **§59-61 Deduplication & References**: `blob_references.rs` — `ReferenceKind`, `ReferenceSet` multiset, and distinct `PinReason`.
   - **§62-66 Storage Lifecycle**: `storage_lifecycle.rs` — `BlobClassification`, `is_gc_eligible` based on unreferenced duration, 5-tier `CacheEvictionTier`, `may_evict` with emergency override, and `StoragePressure`/`effects_for`.
   - **§67-68 Relay & DTN**: `relay_and_dtn.rs` — `RelayStorageAccounting` (keyless storage), `DtnChunkPolicy`, and `chunks_to_send`.
   - **§69-72 Delivery Policy**: `delivery_policy.rs` — `suggested_emergency_priority`, `FileDeliveryTarget`, `OwnDeviceSyncPolicy`, and dual-precondition `BlobAvailabilityAdvertisement`.
   - **§74-77 Source Trust & Manifest Authentication**: `source_trust.rs` — `BlobSource`, and `verify_manifest_authenticity` closing the reorder/replace attack gap by verifying manifests against externally expected `BlobId`.
7. **Round 10 (Transfer Control, Source Integrity & Progress, 2026-09-30)**:
   - **§98-106 Transfer Control**: `transfer_control.rs` — `TransferAckKind`, `ChunkAck` with `ResumeBitmap::received_ranges`, `allows_transfer_completion` enforcing `BlobVerified`, `StagingRetentionPolicy`, `DeferredReason`, and `on_corrupt_chunk` with escalating source penalization threshold.
   - **§118-119 Source Integrity**: `source_integrity.rs` — `SourceSnapshot`, `source_mutated` checking mtime/size/hash changes.
   - **§140-141 Progress & ETA**: `progress.rs` — `TransferProgress` and `estimate_eta` with dedicated `Unknown` type avoiding false precision.
8. **Round 11 (Quarantine, Bitmap Optimization & Resource Policy, 2026-09-30)**:
   - **§91 Quarantine**: `quarantine.rs` — `QuarantineRecord` isolated from normal lifecycle.
   - **§96 Chunk Bitmap Optimization**: `resume.rs` rewrite from `Vec<bool>` to bit-packed `Vec<u64>` (64× memory savings at scale), word-level popcount and completeness checks, plus full complement `received_ranges()`.
   - **§97 Durable Progress**: verified byte tracking confirmed without new type.
   - **§108 & §110-113 Resource Policy**: `resource_policy.rs` — two-tier `RateLimitPolicy`, `FileResourcePolicy`, `BatteryAwareAdjustments`, `MemoryPressureAdjustments`, and `ThermalPressureAdjustments`.
   - **§107, §109, §114**: Multi-source tracking, abuse protection mitigations, and structural ciphertext identity confirmed.

**Verification**: 143/143 unit tests passing in `siar-blob-manifest` (up from 60). `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc --no-deps` clean. Zero regressions across all 5 dependent crates (`siar-crash-recovery`: 50/50, `siar-dtn-bundle`: 56/56, `siar-emergency`: 24/24, `siar-event-registry`: 5/5, `siar-file-transfer-service`: 6/6).

## Known gaps / open questions

- §17 Manifest Hierarchy — explicitly "not required initially" per spec; deferred.
- §33-34 Progressive images/video — pending media-pipeline integration.
- §37-39 Streaming/low-copy I/O and buffer pools, §41-43 parallelism/adaptive concurrency — deferred alongside streaming I/O.
- §47-50 Quotas/sparse files/staging, §51-52 atomic finalization/crash recovery — policy defined in memory, local disk/filesystem integration resides in `siar-storage`/`siar-crash-recovery`.
- §73 Swarming, §78 Merkle Tree Option — explicitly deferred by spec text.
- §79-80 Compression — pending codec dependency decision.
- §92-95 Filesystem permissions & SQL schema — deferred until persistent storage backend implementation.
- §121-139 Platform integration (Android/iOS/Desktop), direct transfer protocol, wire frames — reserved for higher-level transport/UI phases.

## Note

Updated 2026-09-30, recording Rounds 6 through 11 integration from `crates_updated_siar-blob-manifest.tar.gz`. Coverage expanded to ~83/210 sections (~40%).
