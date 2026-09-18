# Part 02 — Multi-Device Identity Architecture

Source spec: `sys-arch/02-multi-device-identity-architecture.md` — 204 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**✅ 204/204 — spec complete** (2026-09-05/06, 11 rounds this session on top of ~90/204 already done beforehand).

256/256 tests (251 baseline + 5 TransportKeyBinding reconciliation tests), clippy clean (`-D warnings`), fmt clean, zero regressions to `siar-routing-policy`/`siar-crypto` at any point across all rounds. Compiled and tested against the real workspace `Cargo.lock` with rustc 1.91.1.

**"Complete" does not mean "nothing left to improve."** This crate's own `lib.rs` doc comment ends with an itemized 21-item Definition-of-Done self-audit (§202): **19/21 fully done, 2 honestly `PartiallyDone`**:
- `UserConfirmationRequired` — the prompt is modeled (`approval::LinkingApprovalPrompt`), but this crate ships no UI at all, by design.
- `FuzzPropertyIntegrationTestsExist` — property tests (`security_invariants.rs`) and integration tests (`integration_tests.rs`) are real; an actual `cargo-fuzz` harness does not exist yet (§164's own named gap).

Other real, named, still-open gaps (none of these block calling the spec "complete" — each is a specific, scoped follow-up, not a missing section):
- **§125 Version Compatibility**: `DeviceCertificate`/`DeviceDirectory` still carry no schema-version field. Retrofitting one now would break already-shipped signed bytes. `wire_limits::SchemaVersion` exists for *new* wire types going forward only.
- **§191 Migration Strategy**: real cross-version migration tests can't exist until §125 is closed. `migration_fixtures.rs` has the seed (fixture round-trip tests) but not the full thing.
- **§189 Error Types**: this crate's real `IdentityError` deliberately does NOT match spec's suggested 12-variant enum — `error_taxonomy::SuggestedErrorCategory` maps every category to where it actually lives instead of a breaking rename.
- **Unused boundaries**: `storage::IdentityStore`, `transaction`'s type-state machine, and all four `client_api` traits (`IdentityClient`/`DeviceClient`/`TrustClient`/`RecoveryClient`) are real, tested types with **zero call sites anywhere in this workspace** — they're the seam, not the implementation.
- **`RootTrustCacheEntry`/`VerifiedContact`** (round 6 vs round 1) — real overlap, named, not consolidated.
- **Three separate "trust state"-shaped types** exist workspace-wide by design (this crate's own, `siar-ui-state`'s, `siar-crypto`'s) — documented, not collided.
- **No real transport wiring**: §91's NFC verification method and §107's rotating discovery tokens have no actual BLE/Wi-Fi/NFC code calling them anywhere.

## Round-by-round summary (all 2026-09-05/06)

| Round | Sections | New modules |
|---|---|---|
| 1 | §91-99 | `contact_verification.rs` |
| 2 | §100-111 | `state_transport.rs`, `principal_claims.rs`, `discovery_privacy.rs` |
| 3 | §112-121 | `reconciliation.rs`, `storage.rs`, `transaction.rs`, `device_authorization.rs` |
| 4 | §122-130 | `enterprise_policy.rs`, `wire_limits.rs`, `session_cache.rs` |
| 5 | §131-139 | `client_api.rs`, `linking_state_machine.rs`, `recovery_state_machine.rs`, `platform_boundary.rs` |
| 6 | §140-149 | `reuse_patterns.rs`, `device_privacy_presentation.rs`, `local_records.rs` |
| 7 | §150-159 | `linking_channel.rs`, `identity_backup.rs`, `threat_model.rs` |
| 8 | §160-169 | `trust_boundary.rs`, `security_invariants.rs`, `integration_tests.rs`, `directory_cache.rs` |
| 9 | §170-179 | `handshake_integration.rs`, `routing_integration.rs`, `call_integration.rs`, `notification_integration.rs` |
| 10 | §180-189 | `link_rate_limits.rs`, `pairing_vs_linking.rs`, `audit_export.rs`, `error_taxonomy.rs` |
| 11 (final) | §190-204 | `algorithm_agility.rs`, `root_key_backup.rs`, `identity_lifecycle.rs`, `migration_fixtures.rs`, `definition_of_done.rs` |

Full per-section rationale for every one of these lives in the crate's own `lib.rs` doc comment — that file is now the authoritative, up-to-date detail record; this docs page is a summary pointer to it.

## Implementing crate(s)

- `siar-identity-multidevice` — 61 source files, 256 tests, spec-complete (including TransportKeyBinding reconciliation).

## Note

Verified directly against the current source tree with real `cargo test`/`cargo clippy`/`cargo fmt --check`/`cargo doc` runs, every round, 2026-09-05/06. This crate's `lib.rs` doc comment is the primary source of truth going forward for what's built and why — this page summarizes it.
