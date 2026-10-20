# Changes since the codebase you uploaded

This archive is the **complete** codebase: your original `siar-crates_tar.gz` layout with every change made since applied. Extract it over (or instead of) your workspace root. Nothing from your upload was removed; **20 files were modified and 42 were added** (exact lists below, produced by comparing this tree byte-for-byte against your upload).

## Read this first

* **`Cargo.lock` is your original, unchanged.** The only package my verified lockfile had that yours lacks is the new workspace crate `siar-files`; every third-party package/version is identical (checked by name and version). Run one normal `cargo build` (not `--locked`) and cargo will add the `siar-files` entry and update the workspace crates' dependency lists. No third-party version moves.
* **I could not build the full workspace.** Your upload did not include `apps/desktop`, `apps/emergency-node` or the Android glue crates that the root `Cargo.toml` lists as members. All my verification was done with those members removed. Please build the full workspace yourself.
* **`workspace-hack` (hakari):** the new crate depends on it, but I did not regenerate it. If your CI verifies it, run `cargo hakari generate` once.
* `ROADMAP.md` (root, new) is the updated roadmap with a per-round ledger of what was built, what was verified, and what is still open and why.

## How it was verified

Last full run (clippy `-D warnings`, all tests, downstream check) exited 0: **693 tests passed, 0 failed** across `siar-crypto`, `siar-blob-manifest`, `siar-file-transfer-service`, `siar-files`, `siar-transport`, `siar-messaging`; `apps/cli`, `siar-event-registry`, `siar-dtn-bundle`, `siar-crash-recovery` compile. Tip: build with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` — the iroh dependency tree fills a small disk in the default debug profile.

Notable test layers: end-to-end over two real iroh endpoints with real device identities; crash injection against the real durable (stoolap) stores; property tests (proptest) on hostile bytes; a hostile-network simulator (loss/duplication/reordering); a bounded-memory large-file test; and red/green regression tests for the bugs below.

## Bugs found along the way (all fixed, all with regression tests)

1. **Durable-store panic**: `reconcile_metadata` asked stoolap for `LIMIT i64::MAX`, which makes its top-N operator reserve that many rows and panic. Post-crash repair would have aborted on the durable store. The in-memory stores hid it.
2. **Stalled transfers hung forever** when a chunk/manifest request was lost; `retry_stalled` now re-requests.
3. **A receiver could not open a file-only transfer**: the key was never delivered. Offers now carry it (raw, for direct end-to-end paths; wrapped per recipient otherwise).
4. **Self-deadlock in the receiver's cancel handling** (introduced by me in an intermediate round): `if let` kept a mutex guard alive across a call that re-locked it. Found by bisection and thread inspection; fixed at both sites; red/green verified.

## Behaviour you should know about

* Revocation (`revoke_access`) stops **future** access only; it cannot recall a key or ciphertext already delivered. The default access ledger is in memory and forgets on restart — plug in a durable `AccessLedger` if revocation must persist.
* A raw key travels inside an offer only on direct authenticated end-to-end connections. Anything crossing a relay/DTN custodian must use `KeyDelivery::Wrapped`.
* Format v2 (chunk-wise AEAD) is opt-in; a v1-only peer cannot decrypt it and the attachment reference carries no version field.
* The domain's `MAX_ATTACHMENT_BYTES` (200 MiB) still caps any described blob well below the 10 GiB manifest limit.

## Provenance note

During one round I found code in the working tree that I had not written in that session (an offer re-send path, a manifest cache, and a messaging attachment-fetch fallback, with tests). I did not rebuild it; I verified, reviewed and fixed a timing bug in one of its tests. It is included here. If it did not come from you, please check where it did.

## Modified files (20)

**(root)**
* `Cargo.toml`

**crates/siar-blob-manifest**
* `crates/siar-blob-manifest/Cargo.toml`
* `crates/siar-blob-manifest/src/descriptor.rs`
* `crates/siar-blob-manifest/src/encryption.rs`
* `crates/siar-blob-manifest/src/fs_store.rs`
* `crates/siar-blob-manifest/src/ids.rs`
* `crates/siar-blob-manifest/src/lib.rs`
* `crates/siar-blob-manifest/src/metadata_store.rs`
* `crates/siar-blob-manifest/src/reference_store.rs`
* `crates/siar-blob-manifest/src/store_error.rs`

**crates/siar-crypto**
* `crates/siar-crypto/src/domains.rs`
* `crates/siar-crypto/src/lib.rs`

**crates/siar-file-transfer-service**
* `crates/siar-file-transfer-service/src/lib.rs`
* `crates/siar-file-transfer-service/src/transfer_engine.rs`

**crates/siar-messaging**
* `crates/siar-messaging/Cargo.toml`
* `crates/siar-messaging/src/blob_bridge.rs`
* `crates/siar-messaging/src/lib.rs`
* `crates/siar-messaging/src/service.rs`

**crates/siar-transport**
* `crates/siar-transport/src/endpoint.rs`
* `crates/siar-transport/src/lib.rs`

## Added files (42)

**(root)**
* `ROADMAP.md`

**crates/siar-blob-manifest**
* `crates/siar-blob-manifest/fuzz/Cargo.toml`
* `crates/siar-blob-manifest/fuzz/fuzz_targets/chunk_frame.rs`
* `crates/siar-blob-manifest/fuzz/fuzz_targets/chunked_decrypt.rs`
* `crates/siar-blob-manifest/fuzz/fuzz_targets/decode_control.rs`
* `crates/siar-blob-manifest/fuzz/fuzz_targets/decode_manifest.rs`
* `crates/siar-blob-manifest/fuzz/fuzz_targets/manifest_builder_equivalence.rs`
* `crates/siar-blob-manifest/src/app_policy.rs`
* `crates/siar-blob-manifest/src/backup.rs`
* `crates/siar-blob-manifest/src/chunked_encryption.rs`
* `crates/siar-blob-manifest/src/file_io.rs`
* `crates/siar-blob-manifest/src/key_wrapping.rs`
* `crates/siar-blob-manifest/src/manifest_builder.rs`
* `crates/siar-blob-manifest/src/parallelism.rs`
* `crates/siar-blob-manifest/src/plaintext_cache.rs`
* `crates/siar-blob-manifest/src/preprocess.rs`
* `crates/siar-blob-manifest/src/scheduling.rs`
* `crates/siar-blob-manifest/src/sharing.rs`
* `crates/siar-blob-manifest/src/wire.rs`
* `crates/siar-blob-manifest/tests/durable_paging.rs`
* `crates/siar-blob-manifest/tests/properties.proptest-regressions`
* `crates/siar-blob-manifest/tests/properties.rs`
* `crates/siar-blob-manifest/tests/scenarios.rs`

**crates/siar-crypto**
* `crates/siar-crypto/src/file_keywrap.rs`

**crates/siar-file-transfer-service**
* `crates/siar-file-transfer-service/src/gc_schedule.rs`
* `crates/siar-file-transfer-service/src/protocol.rs`
* `crates/siar-file-transfer-service/tests/crash_durable.rs`
* `crates/siar-file-transfer-service/tests/crash_injection.rs`
* `crates/siar-file-transfer-service/tests/protocol.rs`

**crates/siar-files** — NEW crate: the public file API (send_file, send_file_streaming, send_file_to_many[_streaming], send_imported, fetch_blob/fetch_attachment pull, import/open/export attachment, offers with key+name carriage and wrapped keys, revoke_access, stall/offer retry, manifest LRU cache, GC maintenance, fair-scheduled concurrent chunk sending)
* `crates/siar-files/Cargo.toml`
* `crates/siar-files/src/access_ledger.rs`
* `crates/siar-files/src/import.rs`
* `crates/siar-files/src/lib.rs`
* `crates/siar-files/src/manifest_cache.rs`
* `crates/siar-files/src/offer_meta.rs`
* `crates/siar-files/src/service.rs`
* `crates/siar-files/tests/end_to_end.rs`

**crates/siar-messaging**
* `crates/siar-messaging/src/files_transport.rs`
* `crates/siar-messaging/tests/fetch_prefers_files.rs`
* `crates/siar-messaging/tests/files_attachment.rs`
* `crates/siar-messaging/tests/files_over_iroh.rs`

**crates/siar-transport**
* `crates/siar-transport/src/files_handler.rs`
