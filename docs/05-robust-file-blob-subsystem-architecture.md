# Part 05 — Robust File / Blob Subsystem Architecture

Source spec: `sys-arch/05-robust-file-blob-subsystem-architecture.md` — 210 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~32/210 (15%) sections.** Partial — approximate, not from a full section-by-section audit like spec 04's (see that file for what such an audit looks like). §20/§21 counted as resolved rather than open (see below), not as new code.

## Implementing crate(s)

- `siar-blob-manifest` — sole implementing crate.
- Cross-references: `events.rs` here implements `04-offline-event-log-architecture.md` §34 File Events, not one of this spec's own 210 sections — tracked in `docs/04`, not here. `siar-crash-recovery`, `siar-dtn-bundle`, `siar-emergency`, `siar-event-registry`, and `siar-file-transfer-service` all depend on this crate.

## Round-by-round history

1. **Baseline (pre-2026-09-01)**: §5-8 (ids: `BlobId`, `ManifestId`, `ChunkHash`, `LogicalAttachmentId`), §9-10 (`descriptor.rs`: `BlobDescriptor`, `FileMetadata`), §11-13 (`chunking.rs`: fixed-size chunking), §14 (`verify.rs`), §15-16 (`manifest.rs`: `BlobManifest`/`build_manifest`), §18-19 (`encryption.rs`: whole-blob AEAD), §26 (`transfer_state.rs`: `TransferState`/`TransferEvent`), §29-30 (`resume.rs`: `ResumeBitmap`). Plus `metadata_encryption.rs`, which is actually Part 28 §31, not one of this spec's own sections — a real cross-spec module living here because the type it encrypts (`FileMetadata`) does.
2. **Phase 3 round 2, 2026-09-19 (see `docs/04` round 3 for full detail)**: §34 File Events, `04-offline-event-log-architecture.md`'s own section, not this spec's — `events.rs`, `EventTypeId` 200-208, `FileEvent` enum lined up with `TransferState`/`TransferEvent`. New `TransferId` newtype added to `ids.rs`. 37/37 tests at the time.
3. **§9 schema-version fix, 2026-09-19 (see `docs/04` round 7)**: `decode_file_event` gained real `schema_version` checking (`FileEventDecodeError::UnsupportedVersion`/`Malformed`), `CURRENT_FILE_EVENT_SCHEMA_VERSION` constant. 38/38 tests.
4. **§56 Durability Classes, 2026-09-25 (see `docs/04`)**: per-`FileEvent`-variant `durability_class()` classification added, part of a cross-domain pass touching all five event catalogs.
5. **§22-25/§27-28/§31-32 cluster, 2026-09-26**: the round documented in detail below.

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

**Verification**: 22 new tests, 60/60 total in `siar-blob-manifest`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean (one auto-format pass applied). `cargo doc` clean (caught and fixed one real broken intra-doc link: `partial_availability` was ambiguous as both a function and a module name). Verified against the real uploaded `Cargo.lock` with rustc/cargo/clippy/rustfmt 1.91.1, installed fresh this round (the sandbox that ran this round started with no Rust toolchain at all). Dependent crates (`siar-crash-recovery`, `siar-dtn-bundle`, `siar-emergency`, `siar-event-registry`, `siar-file-transfer-service`) confirmed to still **build** cleanly against the new code; their full test suites were queued for re-verification but not yet completed in this round — treat "zero regressions" as unconfirmed-but-likely for those five until that run completes, not as verified fact.

## Known gaps / open questions

- §17 Manifest Hierarchy — explicitly "not required initially" per the spec's own text; not attempted.
- §33-34 Progressive images/video, §37-39 streaming/low-copy I/O and buffer pools, §41-43 parallelism/adaptive concurrency, §44-46 file offer/auto-accept/authorization policy, §47-50 quotas/sparse files/staging, and everything from roughly §51 onward — genuinely untouched, a large slice of a 210-section document.
- §22's own two named fields with no home yet in `FileMetadata` (dimensions, duration).
- Full test-suite re-verification of the five real dependent crates after round 5's changes (see "Verification" above) — pending, to be finished before further code work on this crate.
- No blob store exists in this workspace at all — everything here is pure types/decisions; `partial_availability`, `verify`, etc. are the computations a future store would call, not a store.

## Note

Updated 2026-09-27, transcribing round 5's work and reconciling with `docs/04`'s own account of round 2/3's cross-spec event-integration work. Fraction is an approximation (`~32/210`), not a full section-by-section audit — unlike spec 04, this crate hasn't yet had a dedicated tracker-building pass.
