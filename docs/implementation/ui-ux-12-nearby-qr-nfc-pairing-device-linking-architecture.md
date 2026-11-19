# UI/UX Part 12 — Nearby, QR/NFC Pairing & Device Linking UX Architecture

Source spec: `sys-arch/ui-ux-12-nearby-qr-nfc-pairing-device-linking-architecture.md` — 237 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**🟡 Substantially complete (Device Linking & Pairing UI built).**
Implemented in Slint desktop UI (`siar-ui-slint`) and verified both headlessly and live on Wayland display.

## Implementing crate(s)

- `crates/siar-ui-slint` / `apps/desktop` — Security → Devices views, link modals, and notification banners.
- `siar-ui-sync` — `Section::Link` state synchronization between daemon and GUI window.
- `siar-app-core` — link command handlers and status polling.

## Completed UX Features

- **Add a Device Flow**:
  - Displays generated one-time link code (80-bit formatted) and node ticket with one-click copy.
  - Automatically refreshes every 5 seconds; auto-clears on expiration.
- **Pending Approvals Queue**:
  - Lists incoming link requests with device name, public key fingerprint, and explicit Decline / Approve actions.
  - Optional toggle to share verified contacts upon approval.
- **Join Existing Account Flow**:
  - Input fields for ticket and one-time code with format validation.
  - Clear error states for expired or rejected requests.
- **Restart Notice Banner**:
  - Clear global top banner displayed across all app views informing user to restart the application after linking to switch to the linked account identity.
- **Device Management & Revocation**:
  - Displays active devices list with trust status and local device indicator.
  - "Remove" action triggering cryptographic revocation and propagation to contacts.

## Known Gaps / Open Items

- Visual QR code rendering in desktop UI (currently textual code + ticket exchange).
- In-app "Restart Now" button (desktop window cannot directly trigger restart of detached daemon process).
