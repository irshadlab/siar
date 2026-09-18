# apps/android

The consumer every one of `siar-transport-wifi-direct`,
`siar-transport-wifi-aware`, `siar-transport-ble-android`, and
`siar-transport-bluetooth-classic`'s own doc comments named as
missing since they were built: "this workspace doesn't have an
`apps/android` crate yet."

## What's real here

- A working Gradle project skeleton (`settings.gradle.kts`,
  `build.gradle.kts`, `app/build.gradle.kts`, `AndroidManifest.xml`
  with every permission each transport genuinely needs, and why).
- Four Kotlin bridge classes, each driving the real Android SDK API
  for its transport (`WifiP2pManager`, `WifiAwareManager`,
  `BluetoothLeScanner`/`BluetoothGattServer`, `BluetoothSocket`/
  `BluetoothServerSocket`) and calling into the existing Rust JNI
  crates through the exact method signatures those crates' own
  `jni_bridge.rs` files declare — checked field-by-field against the
  real Rust source, not guessed.
- A new Rust crate, `siar-android-connectivity`
  (`apps/android/rust-jni-glue/`), that finally owns a shared
  `siar_domain::ConnectivityState` for the whole app — the specific
  gap every transport crate's doc comment flagged. Real unit tests,
  **compiler-verified** in an isolated sandbox build (2/2 tests pass;
  see this workspace's own memory of this session for how). Caught and
  fixed one real bug this way: `siar_domain::connectivity` is a
  private module, `ConnectivityState`/`TransportLink` are re-exported
  at the crate root instead — the first draft's `use
  siar_domain::connectivity::{...}` failed to compile until fixed to
  `use siar_domain::{...}`.
- `MainActivity.kt` wires all four transports plus the connectivity
  bridge together, requests the right runtime permissions, and shows
  next.md §60's single-line connectivity summary.
- A new Rust crate, `siar-android-messaging`
  (`apps/android/messaging-jni/`), the real `siar_messaging::
  MessageService` FFI surface this README used to name as entirely
  missing. Bootstraps an identity + `SiarEndpoint` (both now persisted
  under `Context.filesDir`, not regenerated fresh each launch), sends/
  receives real text messages (both the `DeviceId`-addressed path and
  the unlinkable token-mailbox path, matching `apps/cli`'s
  `send`/`send-anon`/`check-mailbox`/`check-mailbox-anon` one-for-one),
  reports `TransportLink::InternetDirect` up into
  `siar-android-connectivity`'s shared `ConnectivityState` once its
  `SiarEndpoint` successfully binds, and reports it back down again via
  `shutdown()` from `MainActivity.onDestroy` — see that crate's own
  `lib.rs` doc comment for the exact scope and what's still not
  covered (no groups, no attachments, and `shutdown()` updates
  connectivity state without a full endpoint teardown — see its own
  doc comment for why). `MainActivity.kt` calls `bootstrap()` off the
  main thread (it blocks on real network I/O) and starts polling for
  incoming events.

## What's honestly NOT here

- **iOS.** Explicitly out of scope for this pass — see the
  conversation that produced this module for why. Nothing here assumes
  an iOS app would share this Gradle project; a real `apps/ios` would
  be its own Xcode project with its own bridge layer to the same Rust
  crates.
- **Still no messenger UI.** `siar-android-messaging`/`MessagingBridge`
  can genuinely send and receive messages via both paths now, but
  there's no conversation screen, no contact list, no way to type in a
  peer ticket and see a chat thread — `MainActivity` only shows the
  connectivity summary. `apps/cli`/`apps/desktop` remain this
  workspace's only chat clients with an actual UI. No groups/MLS and no
  attachments are wired into this FFI surface either.
- **`LocalLan`/`InternetRelay` still aren't reported into
  `ConnectivityBridge`.** `InternetDirect` now is (as of this pass, once
  `siar-android-messaging`'s `SiarEndpoint` successfully binds) — the
  other two still aren't, since nothing here can currently tell a real
  direct iroh connection apart from a relayed one or from LAN-local
  discovery (same honest approximation `apps/emergency-node`'s
  `send_and_record` helper makes on the Rust side).
- **Identity/database now persist on both Android and `apps/cli`.**
  `siar-android-messaging` load-or-creates its identity/device id/
  account id and opens a real on-disk database under `Context.filesDir`
  (previously fresh every process start); `apps/cli` gained the exact
  same treatment in the same pass (its own `resolve_data_paths`/
  `load_or_create_id`, mirroring `apps/desktop`'s pre-existing version)
  — all three of this workspace's client entry points now persist
  identity the same way.
- **`LocalLan`/`InternetDirect`/`InternetRelay` are now genuinely
  distinguished**, not defaulted to `InternetDirect` everywhere.
  A local endpoint classifier in `siar-android-messaging` (and
  `siar-connectivity::classify_endpoint_addr`, ported from the retired
  `siar-routing` crate) classifies a peer's *advertised* addresses
  (private/link-local IP → `LocalLan`, public IP → `InternetDirect`,
  relay-URL-only → `InternetRelay`) — real evidence, not a measured path
  (iroh's own `conn_type` API for that was removed upstream; see that
  function's own doc comment for the full story). Wired into both
  `apps/emergency-node`'s `send_and_record` (where the caller happens
  to already have the full address) and `siar-android-messaging`'s
  `send_text`/`send_text_anon`.
- **`.so` build script now exists**: `build-native.sh` runs a real,
  explicitly-scoped `cargo ndk` invocation across all 4 ABIs, for
  exactly the 7 Android-relevant crates — a real build/error report
  against this workspace confirmed that scoping is both necessary
  (`siar-media-audio`/`siar-media-av1`'s C dependencies genuinely can't
  cross-compile for Android without real toolchain setup this
  workspace doesn't own) and sufficient (all 7 crates build cleanly
  across all 4 ABIs). The script still doesn't place `.so` outputs
  directly into `jniLibs/<abi>/` — see its own trailing comment for
  why that step is left for a real run to verify rather than guessed.
- **Real runtime permission checks, not just declarations.** A real
  `./gradlew lintDebug` run against this workspace found 26 lint
  errors this codebase had never actually been checked against before:
  20 `MissingPermission` sites (every Bluetooth/Wi-Fi platform call in
  `BleGattManager`/`BluetoothClassicManager`/`WifiAwareManagerBridge`/
  `WifiDirectManager` now has a real `PermissionsHelper.has*` guard in
  front of it — genuine defensive code, not lint appeasement, since
  these permissions can be revoked by the user mid-session), 5 `NewApi`
  errors (`WifiAwareManagerBridge`'s data-path address extraction needs
  API 29; now version-gated with an early return below that, same
  pattern already used elsewhere in this file for API-gated Bluetooth
  calls), and 1 `CoarseFineLocation` error (the manifest declared
  `ACCESS_FINE_LOCATION` without the `ACCESS_COARSE_LOCATION` API 31+
  requires alongside it — now declared). Also added `NEARBY_WIFI_DEVICES`
  (API 33+'s real replacement for using fine location to gate Wi-Fi
  peer discovery), which the manifest was missing entirely.
- **Android DNS resolution fix**: confirmed directly against iroh
  1.0.3's real published docs (`Endpoint`'s own "Usage on Android"
  section, and `iroh_dns::install_android_jni_context`'s own docs.rs
  page) that iroh's DNS resolver needs a `JavaVM`/Application `Context`
  published to `ndk_context` *before* `SiarEndpoint::bind` is ever
  called, or DNS falls back to Google's public servers (and can panic
  outright under a `panic = "abort"` build profile). `siar-android-
  messaging` now installs this via a `JNI_OnLoad` function — the
  standard, automatic JNI entry point the JVM calls the moment its
  `.so` loads, no explicit Kotlin call needed. One genuine uncertainty
  flagged in that function's own doc comment: it follows the real
  docs.rs example verbatim, but whether `JNI_OnLoad`'s own reserved
  parameter is actually a valid Context in practice wasn't something
  this pass could confirm without a real device/emulator run.
- **Nothing here has been compiled or run.** No Android SDK, Gradle
  wrapper, or emulator exists in this sandbox. Every Kotlin file is
  written against real, current Android SDK API shapes — checked
  against actual documentation wherever genuinely uncertain (see
  `WifiAwareManagerBridge.kt`'s note on `WifiAwareNetworkInfo.getPort()`'s
  undocumented "no port" sentinel) — but none of it has a compiler
  behind it the way the Rust side of this pass did. Please build this
  for real and paste back whatever Gradle/Kotlin compiler errors come
  up, the same loop this whole project has used for every Rust
  delivery.
