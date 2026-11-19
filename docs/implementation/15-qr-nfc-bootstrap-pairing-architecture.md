# Part 15 — QR / NFC Bootstrap & Secure Pairing Architecture

Source spec: `sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md` — 176 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**🟡 Substantially complete (Device Linking flow & Pairing protocol implemented).**
Verified live through CLI and desktop app with real multi-core integration tests.

## Implementing crate(s)

- `siar-identity-multidevice` — device certificates, root authority keys, safety fingerprints, SAS codes.
- `siar-protocol` — `WireMessage::Link` protocol framing (`siar-protocol::link`).
- `siar-app-core` — `linking` state machine (11 tests), `link_flow` orchestrator (4 tests), `contact_devices` (6 tests).
- `apps/cli` (`siar`) — `link-offer`, `link-join`, `link-pending`, `link-approve`, `link-revoke`, `devices`.
- `crates/siar-ui-slint` / `apps/desktop` — Security → Devices pairing UI screens and synchronized `Link` state section.

## Completed Architecture & Protocols

- **One-Time Secure Pairing / Linking Code**:
  - Offering device generates an 80-bit high-entropy code, valid for 5 minutes, single-use.
  - 5 incorrect attempts permanently invalidate and destroy the pending offer.
  - Paired ticket includes transport address and ephemeral bootstrap credentials.
- **Mutual Device Authentication & Certification**:
  - Joining device submits enrollment request with its public signing key and device fingerprint.
  - Approving device displays joining fingerprint in `link-pending` / UI prompt.
  - On approval, root account key signs a `DeviceCertificate` scoped strictly to the new device ID and key.
  - The new device validates that the certificate matches its own identity before adopting the account ID.
- **Persistence & Security**:
  - Linked devices list persisted in `linked_devices.bin` (0600 mode).
  - Account root key kept owner-only (`root.key`, mode 0600) exclusively on the primary authority device.
  - Fully covered by offline and live daemon backup/restore pipelines.
- **Multi-Device Contact Fan-Out & Sync**:
  - Approving device announces new device to paired contacts (`LinkMessage::DeviceAdded`, certificate chain + TOFU root key).
  - Contacts record sibling in `contact_devices.bin` without requiring schema migrations.
  - 1:1 outbound messages and file transfers are fanned out to all active sibling devices.
  - Sent text is mirrored across linked devices.
  - Device revocation (`siar link-revoke` / UI Remove) announces revocation tombstone to contacts.

## Known Gaps / Open Items

- NFC physical transport abstraction (currently executed over Iroh / loopback transport tickets).
- Root key sharing / threshold custody between linked devices (only the authority device currently holds `root.key`).
- Group MLS multi-device fan-out (currently 1:1 text and files fan-out; MLS groups require per-device key package addition).
- History catch-up sync for newly linked devices.
