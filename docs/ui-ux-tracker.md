# Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

No raw `sys-arch/ui-ux-NN-*.md` source documents were uploaded alongside the 33 core architecture specs, so unlike Parts 01-33 this tier has no per-spec source file to point to and no per-spec docs file has been split out — this single file holds the whole tier's tracking, exactly as ROADMAP.md did before this reorganization.

## Implementation status

**1 of 27 specs touched (partially).** This tier was entirely ⚪ before the session that started this work — `apps/desktop` and `apps/android` have substantial pre-existing UI code (chat, groups, attachments) but none of it had been reconciled against the formal 27-spec set until `ui-ux-15`.

| # | Spec | State |
|---|---|---|
| 01 | Product Foundation / Cross-Platform Interaction | ⚪ |
| 02 | Desktop (Dioxus) App Shell / Navigation | ⚪ (`apps/desktop`'s shell pre-dates this spec, unreconciled) |
| 03 | Android (Compose) App Shell / Navigation | ⚪ (`apps/android`'s shell pre-dates this spec, unreconciled) |
| 04 | Conversation List / Inbox | ⚪ (`siar-ui-state::conversation_list.rs` pre-exists, unreconciled) |
| 05 | Message Timeline | ⚪ (`siar-ui-state::timeline.rs` pre-exists, unreconciled) |
| 06 | Composer / Attachments / Voice / Drafts | ⚪ (`siar-ui-state::composer.rs` pre-exists, unreconciled) |
| 07 | Calls / Realtime Media | ⚪ (`siar-calls` crate pre-exists, unreconciled) |
| 08 | Contacts / Requests / Verification / Identity | ⚪ (`siar-ui-state::contact_list.rs` pre-exists, unreconciled) |
| 09 | Groups / Membership / Roles | ⚪ (`siar-ui-state::group_list.rs` pre-exists, unreconciled) |
| 10 | Files / Media Gallery / Transfer | ⚪ |
| 11 | Search / Local Knowledge Retrieval | ⚪ (Part 32 has no crate either — joint gap) |
| 12 | Nearby / QR / NFC Pairing / Device Linking | ⚪ (Part 15 has no crate either — joint gap) |
| 13 | Notifications / Background / Incoming Call | ⚪ (Part 31 has no crate either — joint gap) |
| 14 | Presence / Typing / Receipts / Status | ⚪ (Part 30 has no crate either — joint gap) |
| **15** | **Security Center / Devices / Keys / Recovery** | 🟡 **~83% of 221 total sections** — see detail below |
| 16 | Backup / Restore / Export / Migration | ⚪ (Part 33 has no crate either — joint gap) |
| 17 | Emergency SOS / Offline Mesh | ⚪ (`siar-emergency` crate pre-exists, unreconciled) |
| 18 | Settings / Privacy / Notifications / Data Controls | ⚪ |
| 19 | Plugin/Module Ecosystem | ⚪ (Part 24 has no crate either — joint gap) |
| 20 | Diagnostics / Network Paths / Advanced Dev | ⚪ (Part 18 has no crate either — joint gap) |
| 21 | Accessibility | ⚪ |
| 22 | Design System (tokens/typography/icons/motion) | ⚪ — **candidate for next priority: almost everything else in this tier visually depends on it existing first** |
| 23 | Responsive/Adaptive Layout | ⚪ |
| 24 | Error/Loading/Empty/Offline/Degraded States | ⚪ |
| 25 | Onboarding / First-Run / Permissions | ⚪ |
| 26 | Performance / Virtualization / Large-Data UI | ⚪ |
| 27 | UI Testing / Screenshot / Release Quality Gates | ⚪ |

## `ui-ux-15` detail (the one spec with real work)

§3-90, §96-100, §110-141, §144, §148-183, §186-194 (reconciled) done, ~83% of 221 total sections:

- `RevocationCapabilities`/`sign_out_copy` (§179 — "this signs the device out" only ever renders when actually true).
- The fixed revocation-can't-erase-remote-copies disclaimer (§180-181).
- `RecoveryScope` and its history caveat (§182-183 — silence about unrestorable history would itself be the overclaim these sections warn against).
- **A real spec-internal inconsistency found and documented, not silently resolved**: §184's compromise-response checklist has a different order and two extra steps versus §38's (built in an earlier round). Extended the existing `CompromiseResponseStep` enum with §184's two genuinely new steps (`ReVerifyAffectedContacts`, `CreateFreshBackup`) rather than reordering the original five, since earlier rounds' tests already depend on that order.
- §186-194 closed via reconciliation, no new code — event correlation is explicitly deferred by the spec itself as "advanced future feature"; retention/search/Alerts-vs-Events are policy notes; the four cross-crate integration points (§190-193) were already satisfied by design (`RecoveryStatus`/`BackupSecurityState` reused not duplicated; `DeviceLinked`/`VerificationFailed`/`IdentityChanged` event kinds already exist) or point to still-unbuilt Parts (07 calls, 12 linking is partially built elsewhere, 13 notifications).

**Remaining**: §195-221 — continue in ordinary batches. Next natural slice: the integration section (§195-206, Privacy Settings Integration onward), then testing matrix + final scope (§207-221) — likely finishes the spec in 2-3 more rounds (per ROADMAP.md's own estimate).

## Implementing crate(s)

- `siar-ui-state` — conversation list, timeline, composer, contact list, group list (all pre-existing, unreconciled against their respective specs above).
- `siar-calls` — spec 07.
- `siar-emergency` — spec 17.
- The security-center work (`ui-ux-15`) lives inside `siar-identity-multidevice` (see `docs/02` and `docs/28`).

## Joint gaps (spec pairs where both the core-arch and ui-ux spec are ⚪)

11+32 (search), 12+15 (QR/NFC pairing), 13+31 (notifications), 14+30 (presence/receipts), 16+33 (backup), 19+24 (plugins), 20+18 (diagnostics). Each pair is naturally one unit of work (backend + its UI together), not two separate efforts.

## Note

Transcribed verbatim in structure from ROADMAP.md's Tier 2 section on 2026-09-27. No sys-arch source exists for these specs in this workspace's uploaded scope, so none of this has been re-verified against a raw spec document the way Parts 01-33 can be.
