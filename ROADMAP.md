# siar — Roadmap & Coverage Hierarchy

Maintained alongside the code, not a substitute for it. This file tracks
**what's written vs. what's left**, ordered by actual need — not spec
number order. Update this file every time a spec's coverage changes.

Spec source: `sys-arch/` (33 numbered core-architecture docs +
27 `ui-ux-NN` docs, uploaded once, Wiki-style). Workspace source:
33-crate Rust workspace (`siar-source`) + `apps/{cli,desktop,android,
emergency-node}` + `platform/android`.

Legend: ✅ Done/substantial · 🟡 Partial · ⚪/⬜ Not started · 🔒 Blocked on
a prerequisite · 📋 Reconciled (already covered elsewhere, no new code
needed) · ◇ Conceptual/guidance section, no code artifact of its own.

**2026-09-27 reorganization**: this file used to carry the full
round-by-round narrative and, for spec 04, a complete 95-row
section-by-section tracker inline. That level of detail now lives in
each spec's own `docs/docs/NN-*.md` file — this file is a **map**:
which specs/sections are done, substantial, partial, untouched, or
conceptual, and where to go for the full story. Read a `docs/docs/`
file when you need to know *why* or *how*; read this file when you
need to know *how much* and *where to look next*. `docs/docs/04-*.md`
is the fullest example of what a per-spec file looks like once it has
its own section-by-section tracker — not every spec below has one yet.

---

## Tier 0 — Foundational core crates (Parts 01-09)

All nine have real, compile+test-verified coverage, tracked crate-by-
crate below. Full round-by-round history for every row now lives in
that spec's own `docs/docs/NN-*.md` file (linked in the table) — this
table is a live pointer, not a duplicate of that detail.

| # | Crate | State | Detail |
|---|---|---|---|
| 01 | siar-protocol-ext | ✅ **108/108 — spec complete** | `docs/docs/01-protocol-extension-system-architecture.md` |
| 02 | siar-identity-multidevice | ✅ **204/204 — spec complete** (21-item Definition-of-Done self-audit: 19/21 done, 2 honestly partial) | `docs/docs/02-multi-device-identity-architecture.md` |
| 03 | siar-routing-policy | ✅ **200/200 — spec complete** (19 rounds; own Definition-of-Done self-audit included) | `docs/docs/03-transport-routing-policy-engine-architecture.md` |
| 04 | siar-event-log | ✅ Substantially complete — every section of the spec has been addressed in code, tests or an explicit documented decision. Phase 5 (outbox/effects/retry/reconcile), Phase 6 (scopes/cursors/reassembly; §54 deliberately not built), Phase 7 (snapshots/retention/deletion/backup/upcast), §55-66 closing round, and Phase 8: §27 HLC, §43 BlobRef, §44 search projection, §72 analytics filter, §73-76/§90 reader/appender/SoleWriter, §81 diagnostics, §91 storage-schema stamp, §85/§86 golden + robustness, §87/§80/§88 acceptance scenarios. §93 self-audit (in `definition_of_done.rs`): 14 done, 4 partial, 3 satisfied by other crates. Open: coverage-guided fuzz written but NOT run (needs nightly); no criterion benchmarks; no per-stream checkpoints; cross-process writer lock is the DB's; domain implementations of ReplicationAuthorizer/PayloadSealer; MessageService send path still on siar-storage's OutboxRepository (see siar-messaging::event_policy); apps/desktop projection-only reads unverified. NB: per-section fraction not recomputed (needs docs/docs/04-*.md) | `docs/docs/04-offline-event-log-architecture.md` |
| 05 | siar-blob-manifest | 🟡 **Round 15 (this pass)**: §132-139 `wire` (files/1 control frames, bounded manifest frame, fixed 88-byte chunk header validated before allocation, capability negotiation §133-134), §174-185 `scheduling` (path-change plans, weighted fair scheduler with per-peer/per-app caps, metrics, diagnostics); **wired**: `siar-file-transfer-service::protocol` (`FileReceiver`/`FileSender`, end-to-end test over real stores incl. pause/resume-missing and forged chunks) and `gc_schedule::GcScheduler` (`gc::sweep` now has a caller; `TransferEngine::gc_sweep` protects in-flight blobs). 261 unit + 8 protocol tests, clippy `-D warnings` and fmt clean. ~25 more sections addressed (+§201-205 in round 16); §130/131 deliberately not (no iroh-blobs backend exists). Still open: §115-129 (preprocessing/staging-before-send/handles/platform sources & sinks), §141-147 UX policies, §158-173 (backup, capabilities-based access, forwarding, re-keying), §189-206 (test layers, public API, scope), §41-43. **Round 16 (§201-205, new crate `siar-files`)**: `FilesService` is the public API — `send_file(Destination, source, FileSendPolicy)` → `SendHandle{pause,resume,cancel}`, `next_offer()` → `IncomingOffer{accept,reject}`, `import_attachment`/`bind_to_object`/`open_attachment`, `FileSource` (bytes, path; length checked before allocation, source-mutation detected), driven by one `handle_frame` entry point over a host-supplied `FileChannel`. **siar-messaging now calls it**: `send_imported_attachment` (shared `send_attachment_reference` with the old path) and `FilesBlobStore` (serves the file store to peers; no second copy in siar-storage). Verified over two real iroh endpoints: imported → sent → the recipient's UNCHANGED `fetch_attachment` decrypts it (the two attachment formats are byte-compatible). **Round 17**: `FilesService` now serves chunks through `FairScheduler` (class weights, strict emergency priority, round-robin across peers/apps; replaces the per-request task) and owns GC: `maintenance(now)` / `spawn_maintenance(period)` drive `GcScheduler`. 14 siar-files tests incl. a multi-transfer fairness run and a grace-period GC run. **Round 18 (§115-117, §120-129)**: `file_io` (async `FileSource`/`FileSink`, bounded read that refuses a source that grows or changes, fs source opened once, fs sink atomic/no-overwrite/cleans up), `plaintext_cache` (off by default, byte+age bounded, LRU, zeroized), `preprocess` (fixed §115 order, §116-117 staging decision, `Verified<T>` for §129). siar-files gains `export_attachment` (§125-126; an auth failure writes nothing), `open_verified_attachment`, optional plaintext cache. 25 new blob-manifest tests (286), 13 siar-files tests; clippy/fmt clean; messaging suites unchanged. **Design finding: whole-blob AEAD blocks streaming.** Stream-only send (§117) and memory-free export need a chunk-wise AEAD blob version (e.g. per-chunk nonce from index, tag per chunk); `STREAMING_ENCRYPTION_SUPPORTED=false` makes `StreamOnly` refuse until then — needs a format decision. **Round 19 (chunked AEAD v2 + §142-147, §158-173)**: blob format v2 (`chunked_encryption`: per-segment auth, index-in-nonce, last-flag, authenticated header; single-bit-flip exhaustive test), `ManifestBuilder` (incremental, equals `build_manifest`), `siar-files::import_attachment_streaming`/`send_file_streaming`/segment-by-segment `export_attachment` (memory bounded by one segment; verified: largest read ≤ 256 KiB for a 3 MB file; corruption in a late segment leaves NO file at the destination); `app_policy` (notifications, acceptance prompt, auto-download, data saver, storage classes), `backup`, `sharing`, `key_wrapping`. `FilesService` now takes a `NotificationSink` and an optional auto-download policy (Wi-Fi images accepted unasked, metered deferred to the app) and exposes `IncomingOffer::prompt()`. 328 blob-manifest + 22 siar-files tests; clippy/fmt clean; messaging suites unchanged. Not built: a true stream-to-network driver with no staging (the format now allows it; `decide_staging` says so; no driver); v2 is opt-in because a v1-only peer cannot decrypt it and the attachment reference carries no version; multi-recipient wrapping is not yet connected to a real send (needs the group/session secret source); data saver and storage classes are policy values no scheduler/evictor consumes yet. **Round 20 (§41-43, §191-196)**: `parallelism` (per-path ceilings, resource reductions, AIMD adaptive mode inside the deterministic limit), the `FilesService` pump now keeps N chunk sends genuinely in flight per peer (`set_path_kind`, `set_resources`, `report_sample`; measured LAN>1, Bluetooth==1, battery-saver ≤2), per-peer caps in the scheduler, 11 proptest properties (decoder robustness on hostile bytes, bitmap partition, any-order/duplicate reassembly, corruption detection, v2 mutation rejection, small-file overhead, GC never deletes protected blobs), and a 32 MiB large-file bounded-memory test. 338 blob-manifest + 25 siar-files tests; clippy/fmt clean; messaging suites unchanged. **Round 21 (the §189-198 gaps, written)**: (1) §193 crash injection — `siar-file-transfer-service/tests/crash_injection.rs`: a crash after EVERY possible chunk count (0..=n, incl. all-staged-not-finalized) recovers byte-exact, referenced once, no staging left; journal-behind-disk; torn staging (flipped byte / cut mid-chunk / zeroed page) resumes from exactly the chunks that re-verify; a published-but-damaged blob is never reported complete; GC crash states (file deleted/row kept, row deleted/file kept) are repaired by `reconcile_metadata` with no referenced blob lost, and GC is idempotent. Service-level: receiver dies after the offer (nothing staged/referenced, sender cancels cleanly); lost completion ACK (sender keeps holding the blob; a re-offer costs ZERO chunk frames via dedup). (2) §190 hostile network — the test channel now loses (30%), duplicates (20%) and reorders (≤25 ms jitter) chunk frames deterministically; a 6 MB transfer arrives byte-exact (asserts frames were really lost and duplicated and retransmitted). This exposed a REAL missing feature, now built: `FilesService::retry_stalled` (+ `FileReceiver::request_missing`, run by `spawn_maintenance`) re-requests missing ranges, or the manifest, for a transfer with no progress; a lost manifest request is retried to completion. Also new: `send_imported` (re-offer an already-imported blob under its own reference). (3) §197 Bluetooth scenario — cut off mid-transfer on BLE (exactly one chunk in flight), upgraded to Wi-Fi Direct, resumes with only the missing chunks (frames ≤ chunks+2) and overlaps afterwards. (4) §198 DTN/emergency scenario — policy-layer only (`tests/scenarios.rs`): thumbnail first/original deferred, small critical → DTN, bulk held, DTN parallelism 1, resume of exactly the missing ranges; the DTN TRANSPORT itself (`siar-dtn-bundle`) is not exercised here. (5) §192 coverage-guided fuzzing — 5 cargo-fuzz targets in `crates/siar-blob-manifest/fuzz` (control/manifest decoders, chunk frame, v2 decrypt, builder≡build_manifest); they COMPILE on stable (`cargo check`) but were NOT run — libFuzzer needs `cargo +nightly fuzz run`. Counts: 338 + 11 properties + 2 scenarios (blob-manifest), 6 crash-injection (transfer service), 30 siar-files e2e; clippy/fmt clean; messaging suites and cli unchanged. **Round 22 (durable-store crash injection)**: `siar-file-transfer-service/tests/crash_durable.rs` runs the crash protocol with blob metadata, the reference model and the event log ALL on stoolap files, every handle dropped and reopened from disk: a crash after every chunk count 0..=n (the durable reference survives on its own BEFORE recovery runs, and resume never doubles it), the durable log replays to the pre-crash state, a completed transfer survives restart and a repeat offer dedups, cancel-after-crash releases the durable reference (and stays released across another restart), and the GC crash states (row-without-file, file-without-row) are REAL persisted states repaired by `reconcile_metadata`, with the repair itself durable. **This found and fixed a production bug the in-memory harness could not**: `reconcile_metadata` called `meta.list(usize::MAX)`, and the stoolap-backed stores passed `LIMIT i64::MAX` straight to the engine, whose top-N operator tries to reserve that many rows and panics (`capacity overflow`) — so GC repair after a crash would have aborted on the durable store. Fixed in `StoolapBlobMetadata` (`list`/`list_after` now page internally in 1024-row queries and still honour any limit exactly), `StoolapReferenceStore` (`referenced_blob_ids` pages; `references_for` capped at 16,384 per query), and `reconcile_metadata` (cursor-paged instead of materializing every row). Regression: `tests/durable_paging.rs` (2,600 rows across page boundaries, every limit around 1024/2048, odd cursor page sizes, both durable stores). Counts: 338 + 4 paging + 11 properties + 2 scenarios (blob-manifest), 5 durable + 6 in-memory crash-injection (transfer service), 30 siar-files e2e; clippy/fmt clean; messaging suites and cli unchanged. **Round 23 (files/1 over real QUIC, and pull)**: (1) `siar-transport` gains the `files/1` protocol (`FILES_ALPN`, `FilesProtocolHandler`, `SiarEndpoint::bind_with_files` / `send_files_frame`, `DeferredFilesSink` to break the endpoint↔service construction cycle). Frames are `[kind u8][len u32][bytes]` on ONE persistent unidirectional stream per peer, written under a lock, because `files/1` needs a peer's frames in order and separate QUIC streams are not ordered; lengths are checked before any allocation (`try_reserve_exact`); a failed stream is replaced once. (2) `siar-messaging`: `PeerBook` (device↔endpoint, unpaired endpoints' frames are dropped), `TransportFileChannel`, `FilesInbound`, and `fetch_attachment_over_files`. (3) PULL — the gap that `fetch_attachment` still pulled whole blobs: two appended control messages (`BlobRequest`, `BlobUnavailable`; appended so older peers refuse them cleanly), `FilesService::fetch_blob` / `fetch_attachment`, and a `ServeGate` (default: serve only still-referenced blobs; refusal, absence and unreadable all look identical to the asker). The holder rebuilds descriptor+manifest by streaming the stored blob once (format sniffed from the bytes), offers it under its own send reference, and the requester auto-accepts the offer for exactly the blob it asked for; manifest authenticity and per-chunk/whole-blob hashing still apply, so a wrong blob cannot land under the requested id. Concurrent fetches share one request; a failing redundant transfer does not fail a working fetch; timeouts and local hits handled. Verified over TWO REAL iroh endpoints: push, v2 multi-chunk push, simultaneous both-way transfers, reject frees the sender, unpaired peer dropped, unknown address fails clean and leaves no reference, pull-by-reference with decrypt, pull-unavailable. Counts: 338 + 4 + 11 + 2 (blob-manifest), 11 (transport incl. 5 files-handler), 39 siar-files e2e, 8 files-over-iroh, 14+14+… messaging; clippy `-D warnings` and fmt clean; cli and dependents compile. **Round 24 (the Round-23 "not done" list)** — PROVENANCE NOTE: when I came to this round the working tree already contained the sender-side offer re-send (`retry_unanswered_offers`), the manifest cache, and the `AttachmentFetcher`/`FilesAttachmentFetcher` fallback in `MessageService::fetch_attachment`, with tests, all written after my previous package and not by me in this session. I did not rebuild them; I verified and reviewed them, and the rest of this entry is what I added or fixed. (1) **File key + name carriage (a real hole I had under-reported)**: `send_file` encrypted under a fresh key and never delivered it, so a file-only receiver could verify ciphertext but never open it. Offers now carry the key and an optional display name (`offer_meta`, bounded, parsed defensively — garbage means "no metadata", never an error), the receiver gets `IncomingOffer::display_name()` and, on completion, a `ReceivedFile` (`next_received()`, an UNBOUNDED queue because it holds the only copy of the key) with `reference()`, `display_name()` and `export_name()` (sanitized: no separators, never empty/`..`); a pull carries no metadata (the requester already has the reference). **Security note, documented in `offer_meta`: the key rides inside the offer, which is as private as the authenticated end-to-end QUIC connection it travels on — it must NOT be used over a relay or DTN custodian; use `key_wrapping` per recipient there.** (2) **First-response deadline**: `fetch_first_response_millis` (default 8 s). A peer that never reacts (no offer, no refusal) now fails with `FilesError::NoResponse` quickly, mapped to `Unsupported` so the legacy blob path runs in seconds rather than after the 60 s overall timeout; a peer that did respond but is slow still gets the full timeout (`FetchTimeout`). (3) Fixed a timing bug in the (unattributed) resend test: it waited 250 ms before asserting nothing was re-sent against a 100 ms stall window. (4) Clippy hygiene: the offer parameters are bundled (`BlobToSend`) instead of an 8-argument function. **Verification discipline correction**: earlier in this round I read an empty `clippy` output as success when it had in fact timed out waiting for cargo's build lock behind a background run; I now record explicit exit codes. Final state: clippy `-D warnings` exit 0 over blob-manifest, transfer-service, siar-files, siar-transport and siar-messaging; 586 tests pass across them (siar-files: 59 end-to-end + 5 unit); cli and dependents compile. **Round 25 (wrapped-key and multi-recipient delivery)** — verified the working tree had no changes from anyone else since the Round-24 package before starting. (1) `siar-crypto`: new domain `comm/file-keywrap/v1` and `FileKeyWrapSecret::establish(identity, peer_x25519_public)` — the pairwise secret that wraps blob keys, derived from the same X25519 exchange as the session but through its own domain label (tested independent of the session and mailbox secrets; same static-key forward-secrecy limit as `Session`, inherited, not introduced). (2) `key_wrapping::WrappedKey::{to_bytes, from_bytes}` (strict 60-byte encoding). (3) `siar-files`: offer metadata v2 carries only a key wrapped for ONE recipient (the raw key does not appear in the bytes — tested), `KeyDelivery::{InOffer, Wrapped}` per send, `PairwiseSecrets` trait (host supplies secrets; this crate never sees long-term keys), `IncomingOffer::key_status()` (`Present` / `WrapUnopenable` / `Absent` — a receiver with no or wrong secret still gets the verified bytes and the display name, never a key and never a `ReceivedFile`), and `send_file_to_many`: ONE import, ONE stored blob, one wrapped key per recipient; requires `Wrapped` (the raw key is never sent to several parties); recipients fail independently; each transfer holds its own reference and the import reference is released when offers are out. (4) `siar-messaging`: `PeerBook::register_with_key`, `IdentityPairwiseSecrets` (real identity + recorded peer key; a peer with no recorded key has NO secret — no wrap is ever made against a guess). Verified: 3-device star topology (one blob, both offers name the same BlobId, both recipients open it, the sender's transfer references end at 0); over REAL QUIC with REAL identities: a wrapped file opens on the other device, a key wrapped against a wrong public key does not, a peer with no recorded key gets no wrap. Final state: clippy `-D warnings` exit 0 over siar-crypto, blob-manifest, transfer-service, siar-files, siar-transport, siar-messaging; 671 tests passed, 0 failed (siar-files: 66 end-to-end + 7 unit); cli and dependents compile. **Round 26 (streaming multi-recipient)** — tree verified unchanged since the Round-25 package before starting. `send_file_to_many_streaming`: the same one-blob/one-wrap-per-recipient fan-out, with the v2 chunk-wise format and bounded memory (the fan-out loop is now a shared private helper, so the two entry points cannot drift). Verified over a 3-device star: a 2.8 MB source is never read in a piece larger than 256 KiB, both offers name the SAME v2 blob, both recipients open it, the sender's transfer references end at 0; the wrapped-keys-only rule is enforced on the streaming path too; one recipient failing (no secret) does not stop the other; an empty recipient list leaves nothing referenced. siar-files: 70 end-to-end + 7 unit tests, clippy `-D warnings` exit 0 for siar-files and siar-messaging (the other crates are unchanged since their Round-25 full verification). **Round 27 (revocation, cache pruning — and a deadlock I had introduced)** — tree verified unchanged since the Round-26 package before starting. (1) **Revocation as a service operation**: `FilesService::revoke_access(blob, recipient)` / `restore_access`, backed by an `AccessLedger` trait (default `MemoryAccessLedger`; a host that needs revocation to survive a restart supplies a durable one — the in-memory default forgets). Revoking stops FUTURE access only: no further key is sent (`FilesError::AccessRevoked`), pulls from that device are refused and look exactly like "not here", an unfinished transfer of that blob to it is cancelled; it cannot recall a key or ciphertext already delivered, and the returned `Revocation` says `StopsFutureAccessOnly` with `recipient_may_hold_key` (a test shows a recipient who already had the file can still open it). Revoking one of two recipients leaves the other untouched. (2) **Manifest cache**: now a real LRU (a hit refreshes recency; the FIFO could evict a hot manifest after 16 unrelated inserts) and `maintenance()` prunes entries for blobs GC reclaimed (`prune_manifest_cache`). (3) **BUG FOUND AND FIXED — a self-deadlock that I introduced in Round 24.** When a sender cancelled a transfer the receiver had ALREADY ACCEPTED, the receiver's inbound handler hung forever: `if let Some(b) = self.incoming_blob.lock()….remove(&id) { self.fail_fetch_unless_another_transfer(b) }` keeps the scrutinee's mutex guard alive for the whole block, and the helper locks `incoming_blob` again. I made that edit when swapping `resolve_fetch` for the new helper inside a block rustfmt had reflowed, and it was invisible because every earlier cancel test cancelled before accepting or ended before the receiver processed the cancel. Found by bisecting probe tests (9 variants) and thread inspection (two threads parked on futexes, 0% CPU), not by guessing. Both call sites fixed (bind the removed value first). Regression tests: `cancelling_an_accepted_transfer_leaves_the_receiver_fully_working` and `a_holder_cancelling_a_pull_midway_fails_the_waiting_fetch_promptly`; **red/green verified**: with the bug re-introduced the first hangs (exit 124), with the fix it passes in 0.5 s. Final state: clippy `-D warnings` exit 0 over siar-crypto, blob-manifest, transfer-service, siar-files, siar-transport, siar-messaging; 693 tests passed, 0 failed; cli and dependents compile. Still not done: a wrapped key has no revocation story at the service level (`key_wrapping::revoke` exists but nothing calls it — and it can only stop future wraps, as documented); a multi-recipient send does not de-duplicate against a recipient that already holds the blob; (older, now mostly historical) `MessageService::fetch_attachment` itself still uses the old `BLOB_ALPN` path — an app switches by calling `fetch_attachment_over_files` (they take the same reference and peer; the old path is kept for peers without files/1); no encrypted-metadata/filename carriage on offers; the manifest rebuilt for a pull is streamed from disk on every request (no cache). Earlier still-open: sender-side re-send of an unanswered offer (a session concern); network loss at the QUIC level beyond what the simulator models; `references_for` silently caps at 16,384 rows per call (documented; a blob referenced more often than that is not a realistic case, but it is a cap). Honest gaps (older): `spawn_maintenance` exists but no app calls it yet; the receive side of `fetch_attachment` still pulls the whole blob through siar-transport rather than `files/1` chunks (the engine path is exercised in siar-files' own tests, not yet in the app transport); `siar-domain`'s 200 MiB `MAX_ATTACHMENT_BYTES` caps any described blob far below `ManifestLimits`' 10 GiB, so bigger files are refused until that constant is decided; import holds the file in memory once (whole-blob AEAD); no send-side scheduler use yet (`FairScheduler` exists, `FilesService` does not call it). Earlier: Round 13 added the local blob store (§37, 39, 40, 47-52, 86-90, 186-188, §38 partly). **Round 14**: §148-157 reference store + GC + staging retention (+ §150 recount), §154 typed expiries, §79-81 compression with the codec decision made (DEFLATE/flate2 pure-Rust) and a tested bomb defence, §82-85 derivatives/voice/channel separation; **and the store is now wired into `siar-file-transfer-service`** (`transfer_engine`: receive/pause/resume/cancel/restart/late-completion, + `restore_from_log`). Still open: ~38 sections, mainly §141-147/§158-185 (acceptance UX, backup, capability/forwarding/re-keying, multipath, fairness, diagnostics), §190-209 (test layers, public API, definition of done), §41-43 parallelism, §83 thumbnail *generation* (needs an image codec). NOT done: send-side engine; siar-messaging does not call any of it; nothing schedules gc::sweep; the old '~102/210' was a rough guess — recount with grep | `docs/docs/05-robust-file-blob-subsystem-architecture.md` |
| 06 | siar-dtn-bundle | 🟡 **~83/192** (+18 in round 5: §29-31, 37-39, 71-72, 80-81, 107-108, 142-144, 154-156; 119 tests) | `docs/docs/06-dtn-store-carry-forward-architecture.md` |
| 07 | siar-capability | 🟡 **~113/164 (by section references in source; the old ~19/164 predates rounds N+1..N+3)** — round N+3 +15: §57-60, 75-76, 84-86, 97-98, 104-107 (UI adapter, lazy/hierarchical negotiation, failure policy, directional capabilities, trust separation, resume); 122 tests (114 unit + 8 property). Remaining: §1-3 overview, 71, 77, 79-83, 87-88, 99, 115-117, and §122-164 (testing, phases, definition of done) | `docs/docs/07-capability-negotiation-architecture.md` | `docs/docs/07-capability-negotiation-architecture.md` |
| 08 | siar-resource-limits | ✅ **~192/193 — §187-188 facade (`ResourceRuntime`) done, Definition of Done (§191) executable and passing; §186/190 mapped in the crate README** (round 13: facade + 4 fixes, real cargo-fuzz harness, multi-sample benchmark; round 12: §45, 169-171, 172-185, 189, 191 — storage classes, error classes, property/concurrency/scenario/fuzz tests, benchmarks; remaining: §187-188 public facade/integration API once the wiring settles, plus prose-only §1-2, 186, 190, 192-193; +13 in round 11: §113-120, 123, 129-130, 156-157 — per-feature limits, pre-allocation frame decoder, unknown-peer + connection admission pipelines, sharded scheduler; +15 in round 10: §80, 105-110, 137-140, 165-168 — snapshot, bounded-cardinality metrics, user/dev feedback, batched events + journal filter, restart/crash recovery planning; +13 in round 9: §86-89, 121-122, 146-152 — CPU/thermal/energy, effective limits + runtime tightening, composable runtime modes; +15 in round 8: §68, 70-77, 82, 84-85, 127-128, 131 — routing/discovery/negotiation limits, event-log/projection/outbox pressure, IPC/FFI/plugin fan-out queues, verification budget + handshake pools, OS memory pressure; +14 in round 7: §28, 98, 144-145, 153-155, 158-164 — account tree/tenant isolation, fair semaphore, preemption+safe pause, cancel scopes, task registry, supervisor, sharded stats; +14 in round 6: §5-7, 9-16, 141-143 — layers/hard+soft limits, global budget + borrowing, profiles, config validation, dynamic config; +~15 in round 5: §42-43, 62-67, 78-79, 111-112, 132-136; round 4 added §58-61, 69, 81/83/87, 91-97, 99-104, 124-126, 129, 169-171; 434 tests across unit/property/concurrency/scenario/fuzz/audit/DoD/facade) | `docs/docs/08-resource-limits-backpressure-architecture.md` |
| 09 | siar-crash-recovery | ✅ **186/186 — all sections implemented or consciously not built (§10 WAL = documented do-not-build; §182's three deferrals recorded)**; Definition of Done (§184) is an executable audit: 16 Done / 4 PartiallyDone with the missing piece named, 0 failing; §186 final principle verified clause by clause. Closing round (§180-186): `RecoveryFailure` (spec's 10-variant error enum, replaced the coordinator's invented 5), `scope_and_phases` (§180/182/183/185 as data checked against the real source tree and Cargo.toml), `definition_of_done`. **Honest gap: no other workspace crate depends on `siar-crash-recovery` yet** (`measure_wiring` reports it; wiring the coordinator into the node runtime is the next integration task). 495 unit + 3 integration tests; clippy `-D warnings` clean (test, release, `crash-injection`); `cargo doc -D warnings` clean | `docs/docs/09-crash-recovery-architecture.md` |

**Three real, unresolved reconciliation questions**, deliberately not
silently resolved — documented in the newer crate's own `lib.rs` in
each case, and tracked in detail in `MIGRATION.md`:
- two device-cert models (`siar_crypto::device_cert` vs
  `siar-identity-multidevice`)
- two routing/scoring systems (`siar-routing` vs `siar-routing-policy`)
- two DTN bundle models (`siar-dtn` vs `siar-dtn-bundle`)

---

## Cross-spec build order (Parts 01-33) — dependency analysis

Built 2026-09-27, at the user's own request, to answer a real question:
after Tier 0, in what order should the remaining 24 core specs be
built to minimize having to come back and rework something because a
prerequisite showed up late?

**Method**: every `sys-arch/NN-*.md` file was grepped for `Part NN`
cross-references to every other spec. This is a real measurement, not
a guess — but it's a heuristic, not a verified hard-blocking graph:
most of these mentions are "this relates to / this is consumed by,"
not a compiler-enforced dependency, so treat the resulting order as a
strong prior to plan around, not a rule that can never be reordered.
**In-degree** (how many *other* specs' own text points at a given
spec) is the signal that matters most for sequencing: a spec many
others reference is one whose absence forces the most future rework if
built late, regardless of how many things *that* spec itself
references (a spec can reference dozens of "related areas" without any
of them being a real prerequisite for it).

### Full in-degree ranking (excluding self-references)

| Part | In-degree | Status | Part | In-degree | Status |
|---|---|---|---|---|---|
| 08 Resource Limits/Backpressure | **22** | 🟡 Tier 0 | 14 Proximity Abstraction | 8 | ⚪ |
| 10 Fuzzing/Test Suite | **19** | ⚪ — see note below | 15 QR/NFC Pairing | 7 | ⚪ |
| 13 Battery-Aware Scheduling | **18** | ⚪ | 11 Relay Self-Hosted Infra | 6 | ⚪ |
| 03 Transport Routing Policy | 17 | ✅ Tier 0 | 22 WASM Components | 6 | ⚪ |
| 16 Daemon/Headless Runtime | 15 | ⚪ | 01 Protocol Extension | 5 | ✅ Tier 0 |
| 19 C-ABI/FFI | **15** | ⚪ — see note below | 21 3rd-Party Protocol Ext | 5 | ⚪ |
| 04 Offline Event Log | 14 | 🟡 Tier 0 | 28 Security/E2EE | 4 | 🟡 Tier 1 |
| 05 File/Blob Subsystem | 14 | 🟡 Tier 0 | 29 Realtime Calls | 2 | ⚪ |
| 07 Capability Negotiation | 14 | 🟡 Tier 0 | 25 Android HW Media | 1 | ⚪ |
| 17 Emergency Priority | 13 | 🟡 (partial crate) | 26 Audio DSP | 1 | ⚪ |
| 18 Network Diagnostics | 13 | ⚪ — see note below | 27 Android Build Automation | 1 | ⚪ |
| 12 Multipath Networking | 11 | ⚪ | 31 Notifications | 1 | ⚪ |
| 23 External Interop | 11 | ⚪ | 32 Search/Indexing | 1 | ⚪ |
| 02 Multi-Device Identity | 10 | ✅ Tier 0 | 30 Presence/Typing | 0 | ⚪ |
| 09 Crash Recovery | 10 | 🟡 Tier 0 | 33 Backup/Restore | 0 | ⚪ |
| 20 Embedded Linux Node | 10 | ⚪ | | | |
| 24 Plugin/Module Ecosystem | 10 | ⚪ (deliberately last) | | | |
| 06 DTN Store-Carry-Forward | 9 | 🟡 Tier 0 | | | |

**Two ranking corrections — high in-degree that should NOT jump the
queue:**
- **Part 10 (Fuzzing/Test Suite)** ranks 2nd, but it isn't a phase to
  schedule — it's a *practice* applied to each crate as it's built
  (exactly what this workspace already does: `siar-event-log`'s crash
  injection/property tests, `siar-blob-manifest`'s tamper tests). Its
  high in-degree just reflects how many specs say "add fuzz/property
  tests per Part 10," not that Part 10 has to exist as its own crate
  before anything else can proceed.
- **Part 19 (C-ABI/FFI)** ranks in the top 5, but binding generation
  against an API that hasn't stabilized yet is exactly the kind of
  back-and-forth this exercise is trying to avoid — build bindings
  once the crates underneath them stop shifting, not before. Treat as
  deliberately late despite the raw score.
- **Part 18 (Network Diagnostics)** has real in-degree, but reads the
  other way around: it *visualizes and diagnoses paths across
  everything else* (routing, DTN, multipath, proximity) — it needs
  those to exist first to have anything to show, so it naturally sits
  after most of the networking-layer specs, not before them.

### Recommended order after Tier 0

Tier 0 (01-09) continues in the standing order (finish 06 → 07 → 08 →
09) — the ranking above *confirms* that plan rather than overriding
it: 07 and 08 are the single highest true "build this or everything
downstream reworks around its absence" items in the entire 33-spec set
outside what's already done.

1. **13 Battery-Aware Scheduling** — 2nd-highest real (non-corrected)
   in-degree; a cross-cutting policy layer nearly every
   networking/background spec below needs to respect (duty-cycling,
   scan frequency, discovery aggressiveness).
2. **16 Daemon/Headless Runtime** — defines how the whole system runs
   without a UI; `apps/emergency-node` and any embedded/daemon work
   depend on this existing first.
3. **12 Multipath Networking** — extends Part 03 (done) directly; nothing
   new to wait on.
4. **17 Emergency Priority Classes** — already has real partial crates
   (`siar-emergency`, `siar-emergency-service`) from the Tier 0 event-
   integration work; continuing it is lower-effort than starting cold.
5. **14 Proximity Abstraction** — a named prerequisite for #6 below.
6. **15 QR/NFC Bootstrap Pairing** — **this is the concrete reason Part
   28 isn't next**: Part 28's own tracked gap (§37-39 SAS/pairing, no
   `protocol_version` field on `DeviceLinkInvite`) is a pairing-flow
   problem, and Part 15 is where the real pairing flow this gap needs
   to reconcile against actually gets built. Doing 28's own pairing
   work before 15 exists means guessing at a shape that would likely
   need reworking once 15 lands.
7. **28 Security/E2EE** — resume here, now with Part 15 as real
   infrastructure to reconcile the SAS/pairing gap against, rather than
   an assumption.
8. **11 Relay Self-Hosted Infra**, then **18 Network Diagnostics**
   (now that most of the networking layer above actually exists to
   diagnose), then **23 External Interop**, **20 Embedded Linux Node**.
9. **29 Realtime Calls** (wants Part 28's ratchet/groups reasonably
   solid first), then **30 Presence**, **31 Notifications** (both
   reference 28 and 29), **32 Search**, **33 Backup** (references the
   most other specs of anything in the set — 28, 31, 32 among them —
   and has zero in-degree itself, the textbook "build last" signature).
10. **25/26/27** (Android hardware media, audio DSP, build automation)
    — platform-specific, slot in whenever Android work is prioritized
    independently of this ordering.
11. **19 FFI**, last among the "core" specs, once the APIs it binds
    have actually stopped changing.
12. **21/22/23/24** (third-party protocol extensions, WASM, plugin
    ecosystem) — already the standing call in "Suggested next
    priorities" below: extensibility/ecosystem work is deliberately
    lowest priority until the core product is solid, and this ranking
    doesn't change that call.

---

## 2026-10-02 refactor/audit pass

Workspace-wide `cargo clippy --all-targets` is warning-free; `cargo test`
(excluding the heavy `siar-media-av1`/`siar-media-audio` codec crates)
had 1697 passing / 0 failing at last count. Defects fixed:
- 06: `decide_forwarding` cast peer count with `as u8` (256 peers → 0 →
  every bundle held forever); `StoredBundle::peer_history` unbounded
  (§143); `retry_count` overflow; payload-hash compare now constant-time.
- 07: `BoundedBytes` derived `Deserialize`, bypassing its 256-byte bound
  for peer-supplied bytes.
- Saturating counters: blob transfer journal retries, UI unread count,
  identity link rate limiter.
- 08: `TokenBucket` moved its refill mark backwards on a regressing clock,
  granting a free burst on the next forward reading; strict-priority
  `BoundedPriorityQueue` can starve low tiers (new `AgingQueue` is the
  starvation-safe alternative; the old queue is kept as-is).
- 08 (round 8): `BoundedCounter::release` used a plain `fetch_sub`, so an
  over-release would wrap the counter to ~`u64::MAX` and deny everything;
  it now saturates (and asserts in debug builds).
- 08 (round 10): design finding, not a code bug — `ReservationId` is a
  per-process counter restarting at 1, so it must never be persisted;
  `restart.rs` keys durable records on `OpId`/`DurableResId` instead.
- 08 (round 11): the new cross-validation caught a real default error
  (Embedded DTN max bundle 1 MiB > hard frame limit 256 KiB, i.e. a
  bundle that could be accepted by policy but never received).
- 08 (round 12), found while writing the Definition-of-Done tests: (a)
  `PeerQuota::for_trust_class` was device-independent, so one trusted
  peer's quota could exceed a whole Embedded node's stream budget — added
  `clamped_to`; (b) §191 requires separate durable/cache/staging/relay
  quotas but no cache quota or class ledger existed — added
  `storage_classes`; (c) `ResourceError` lacked most of §169's variants.
  Adding variants to `ResourceError` can break exhaustive `match`es in
  callers (compile-time, easy to fix).
- 08 (round 13): `OutboxAccounting` charged critical entries sitting in
  the reserve against *normal* capacity (freeing a normal slot gave none
  back); `plan_recovery`/`restore_reservations` could place one durable id
  in two buckets and restore the wrong record when ids repeated (found by
  the new fuzz target on its first corpus run); `fuzz_entry` needed a
  dev-only dependency in normal builds (found by building the real fuzz
  targets — test/clippy `--all-targets` builds mask missing normal deps,
  so a plain `cargo check --workspace` is now part of the checklist).
  `ResourceError`, `Retryability`, `RetryClass`, `ResourceCause` and
  `UserMessage` are `#[non_exhaustive]`: downstream exhaustive `match`es
  need a wildcard arm once.
- 07 (round N+3): `HierarchicalSession::finish` honoured results for
  extensions never begun / unsupported, so a stray `Ok` made an
  unsupported extension usable and a forged fatal `Err` could terminate a
  healthy session (property test with the intended invariant found it).
  Also noted: a generic bitset intersection is wrong for directional
  capabilities (AND keeps "send" between two send-only peers); they use
  `direction::negotiate_directional`.
Not touched: the three open reconciliation questions above.

---

## Tier 1 — Security backbone (Part 28)

**~46/127 (36%) done.** Four remaining items are each their own
subsystem (the ratchet, group security/MLS reconciliation, a security
test-vector/fuzzing harness, a 10-crate workspace split) rather than a
next incremental batch — recommendation is to pause here until one of
those four is deliberately prioritized. Full detail, the closed/open
section ranges, and the four-item table: `docs/docs/28-production-security-e2ee-key-management-privacy-architecture.md`.

---

## Desktop UI toolkit change: Dioxus → Slint (2026-10-22)

The desktop shell no longer uses Dioxus (WebView). It is now Slint, in a new
crate `crates/siar-ui-slint`; `apps/desktop` is a thin core-wiring shell.
Android stays Jetpack Compose, unchanged. The `ui-ux-NN` specs describe the
*design* (shell, tokens, states); only their Dioxus-specific implementation
wording (signals, rsx) is superseded.

**Done and verified** (40 UI tests + 123 ui-state tests + desktop tests, all
green; plus an end-to-end test with two real cores — pair, send, receive,
ACK → Delivered):
- `ui-ux-22` design tokens as the `Theme` global (semantic roles, light/dark/
  high-contrast, density, UI scale, window classes); components consume
  semantic tokens only.
- `ui-ux-02` shell: rail, adaptive sidebar/content (one pane on compact
  windows), status bar, native window decorations.
- Chats (`ui-ux-04/05/06` subset): conversation list + filter, bottom-anchored
  timeline, image previews, honest "carried by peers — not delivered" state,
  composer (Enter / Shift+Enter).
- Contacts, groups + invites + add-member, connect/pairing, settings
  (persisted), security events + banner + devices (`ui-ux-15`).
- Core wiring fixes vs the Dioxus shell: one stable conversation per peer,
  per-conversation timelines, frames routed by sender, real delivery ticks
  (storage poll), user-visible errors instead of log-only.

**Not done / known gaps**
- "Load earlier messages": needs a decrypting history accessor in the core.
- Calls, Files, Search destinations (no backend yet).
- Not run on a real display/GPU (software-renderer + headless tests only).
- Network "Online" means the transport endpoint is bound, not probed.
- Slint pinned `=1.16.1` (1.17+ needs rustc 1.92; workspace is on 1.91).
- Run `cargo hakari generate` (workspace-hack lost its Dioxus-only entry).

## Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

**1 of 27 specs touched (partially)** — `ui-ux-15` Security Center is
~83% of 221 sections; the other 26 are ⚪, several sharing a "joint
gap" with a still-unstarted core-arch spec (search, QR/NFC pairing,
notifications, presence, backup, plugins, diagnostics). `ui-ux-22`
Design System is the standing next-priority candidate, since most of
the rest of this tier visually depends on it existing first. Full
27-spec table and `ui-ux-15`'s own detail:
`docs/docs/ui-ux-tracker.md`.

---

## Tier 3 — Toolchain / sandbox environment note

Historical constraint, now resolved: this sandbox's verification
environment used to be stuck on rustc 1.75 with no path to the
workspace's pinned `rust-version = "1.91"`. **Correction (still
current)**: `rustc-1.91`/`cargo-1.91`/`rust-1.91-clippy`/`rustfmt-1.91`
install directly via `apt-get install` (`archive.ubuntu.com` is
allowlisted), landing at `/usr/lib/rust-1.91/bin`. Combined with the
right system libs (`libgtk-3-dev libssl-dev libsoup-3.0-dev
libjavascriptcoregtk-4.1-dev libwebkit2gtk-4.1-dev libasound2-dev
cmake libopus-dev libdav1d-dev`), `cargo check --workspace
--all-targets` against the real repo-root `Cargo.lock` (v4 format —
parses fine under cargo 1.91; don't delete/regenerate it) passes clean
across all 33 crates, including `apps/desktop`. Disk space is tight —
expect to `rm -rf target && apt-get clean` between big check runs. Only
genuine device/emulator/hardware-codec behavior (`apps/android`'s real
Kotlin/JNI runtime, hardware codecs) remains out of reach here.

Each new sandbox session for this project starts with **no Rust
toolchain installed at all** (not even 1.75) — install fresh every
time; never assume a prior session's toolchain state still holds.

---

## Suggested next priorities, in order

Tier 0's nine core specs are being worked through one by one, in spec
order, per standing user instruction, before moving on. Specs 01-03
are complete. Spec 04 has Phases 1-4 done, Phase 5 deliberately
deferred (see `docs/docs/04-*.md`). Spec 05 is 2 rounds into an
ordinary section-by-section pass (see `docs/docs/05-*.md`). Specs
06-09 remain untouched since their last recorded aggregate count.

**Once Tier 0 is done, the "Cross-spec build order" section above is
the primary sequencing for Parts 10-33** — it supersedes the older,
less rigorous list this section used to carry. The two tracks below
run alongside it, not instead of it:

- **UI/UX**: `ui-ux-15` continues in ordinary batches (§195-221
  remain — integration section §195-206 next, then testing/final scope
  §207-221). `ui-ux-22` Design System is worth pulling forward once
  Tier 0 ends, ahead of most core-arch work above it in the ranking —
  everything else in that tier visually depends on it, and no core-arch
  ranking captures that (`ui-ux` specs weren't part of the reference-
  graph measurement). The "joint gaps" pattern noted before (search,
  QR/NFC pairing, notifications, presence/receipts, backup, plugins,
  diagnostics — each a core-arch spec paired with its own ⚪ ui-ux
  counterpart) still holds; the cross-spec order above already
  sequences the core-arch half of each pair, so build that spec's own
  ui-ux counterpart in the same window rather than as a separate later
  effort.
- **Deliberately last, by explicit priority call**: plugin/module
  ecosystem (Part 24, ui-ux-19), WASM components (Part 22), third-party
  protocol extensions (Part 21), external interoperability (Part 23).
  Usability and reliability come first; extensibility/ecosystem work
  is lower priority until the core product is solid — the cross-spec
  ranking's own in-degree numbers agree (all four rank in the bottom
  third).
- **The four Part 28 subsystems** (ratchet, groups/MLS, test harness,
  crate split) — each needs its own dedicated, scoped effort rather
  than sharing a round with anything else, whenever Part 28 is resumed
  per the cross-spec order above.
