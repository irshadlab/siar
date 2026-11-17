# UI/UX Part 15 — Security Center, Devices, Keys & Recovery UX Architecture

Source spec: `sys-arch/ui-ux-15-security-center-devices-keys-recovery-architecture.md` — 221 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**184/221 (83%) sections.**

**~83% done**, rounds 7-18. §3-90, §96-100, §110-141, §144, §148-183, §186-194 closed. 123/123 tests, zero regressions. Built entirely in siar-ui-state (dependency rule: siar-domain-only, never siar-crypto).

## Implementing crate(s)

- `siar-ui-state (device_lifecycle.rs, security_event.rs, identity_verification.rs, recovery.rs, recovery_advanced.rs, security_status_banner.rs, privacy_and_lock.rs, revocation_lifecycle.rs, diagnostics.rs, presentation_api.rs, ui_effects.rs, accessibility.rs, empty_states.rs, revocation_honesty.rs)`


## Known gaps / open questions

- Real spec-internal inconsistency found and documented, not silently resolved: §184's compromise-response checklist has a different order and two extra steps vs §38's (built earlier) — extended `CompromiseResponseStep` with the two genuinely new steps rather than reordering, since earlier tests already depend on the original order.

- §186-194 closed via reconciliation with no new code: event correlation is explicitly deferred by the spec itself as an 'advanced future feature'; the four cross-crate integration points were already satisfied by design or point to still-unbuilt Parts (07 calls, 12 linking, 13 notifications).

- Remaining: §195-221 (Privacy Settings Integration onward, then the testing matrix + final scope) — paused while Tier 0 core specs are worked per current priority, ~2-3 more rounds when resumed.


## Note
Detail above is transcribed from ROADMAP.md and project working notes as of 2026-09-01. This file has not yet been independently re-verified line-by-line against the current source tree — treat the section fraction as the last real count, not a live measurement.
