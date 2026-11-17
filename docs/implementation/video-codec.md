# Multi-Platform Desktop & Web Release, Packaging, Code Signing & Distribution Architecture

> **Scope Note:** Realtime video codecs, zero-copy Android MediaCodec surfaces, audio DSP, and call control protocols are specified in [`sys-arch/25`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md), [`sys-arch/26`](file:///home/irshad/Projects/siar/sys-arch/26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md), and [`sys-arch/29`](file:///home/irshad/Projects/siar/sys-arch/29-realtime-calls-media-session-protocol-architecture.md). Android native build packaging automation is specified in [`sys-arch/27`](file:///home/irshad/Projects/siar/sys-arch/27-rust-driven-android-native-build-packaging-automation.md). This document defines the multi-platform desktop release packaging, code signing, notarization, automated updating, and distribution pipeline.

---

## 1. Multi-Platform Release Artifact Matrix

```text
                                 Git Tag (v1.0.0)
                                        │
                               CI Multi-OS Matrix
                                        │
             ┌──────────────────────────┼──────────────────────────┐
             ▼                          ▼                          ▼
      Linux Builder              Windows Builder             macOS Builder
     (Ubuntu Runner)            (Windows Runner)            (macOS Runner)
             │                          │                          │
   ┌─────────┼─────────┐                │                          │
   ▼         ▼         ▼                ▼                          ▼
AppImage   .deb      .rpm          .exe / .msi                   .dmg
(x86_64, (Ubuntu,  (Fedora,       (Authenticode               (Apple Signed
aarch64)  Debian)   RHEL)            Signed)                   & Notarized)
   │         │         │                │                          │
   └─────────┼─────────┴────────────────┴──────────────────────────┘
             ▼
    SHA256 / BLAKE3 Checksums + GPG Signature
             ▼
    Signed Release Manifest (Auto-Updater)
             ▼
    Public Website / Direct CDN Distribution
```

### Artifact Summary

| Platform | Architecture | Target Formats | Signing / Integrity Mechanism |
| :--- | :--- | :--- | :--- |
| **Linux** | `x86_64`, `aarch64` | `.AppImage`, `.deb`, `.rpm` | GPG detached signature, SHA256SUMS |
| **Windows** | `x86_64`, `aarch64` | `.exe` (Installer), `.msi` (Enterprise) | Microsoft Authenticode (EV Certificate / HSM) + RFC 3161 Timestamp |
| **macOS** | `Apple Silicon (arm64)`, `Intel (x86_64)` | Universal `.dmg` / `.app` | Apple Developer ID Application + Hardened Runtime + `notarytool` Notarization |
| **Android** | `arm64-v8a`, `x86_64` | `.apk` (Direct), `.aab` (Play Store) | Play App Signing (Play) / Release Key (Direct APK) (see [`sys-arch/27`](file:///home/irshad/Projects/siar/sys-arch/27-rust-driven-android-native-build-packaging-automation.md)) |

---

## 2. Linux Desktop Packaging & Runtime Architecture

Dioxus Desktop on Linux utilizes the system WebView (`WebKitGTK`). Packaging must correctly bundle and specify runtime dependencies.

### Formats & Packaging
1. **AppImage (`messenger-x86_64.AppImage`, `messenger-aarch64.AppImage`):**
   - Primary self-contained generic download for Linux distributions.
   - Bundles required shared libraries while dynamically binding to system `glibc` and display servers (X11 / Wayland).
2. **Debian / Ubuntu Package (`.deb`):**
   - Packages metadata, desktop entry (`.desktop`), scalable vector icons, and specifies explicit package dependencies:
     ```control
     Package: siar-messenger
     Architecture: amd64
     Depends: libc6 (>= 2.34), libgtk-3-0, libwebkit2gtk-4.1-0 | libwebkit2gtk-4.0-37
     ```
3. **Fedora / RHEL Package (`.rpm`):**
   - Standard RPM spec with dependency on `webkit2gtk4.1` and `gtk3`.

### Supported Linux Distributions
- Ubuntu LTS (22.04, 24.04+)
- Debian Stable (12+)
- Fedora Current / Current-1
- Arch Linux

---

## 3. Windows Release & Authenticode Signing

### Packaging Formats
1. **Setup Executable (`MessengerSetup-x86_64.exe`):**
   - Built via InnoSetup or NSIS containing the compiled Rust binary, assets, and WebView2 bootstrapper.
2. **Enterprise Windows Installer (`Messenger-x86_64.msi`):**
   - WiX-generated MSI package supporting unattended silent installation and GPO enterprise deployment.

### WebView2 Runtime Policy
- Modern Windows 10 (20H2+) and Windows 11 include evergreen WebView2 runtime by default.
- The installer detects WebView2 presence and downloads the evergreen bootstrapper if missing.

### Code Signing Pipeline
```text
Compiled Binary (messenger.exe)
             │
             ▼
   Authenticode Signing Tool (signtool.exe)
   - SHA-256 Digest Algorithm
   - RFC 3161 Timestamping Server (DigiCert / Sectigo)
   - Hardware Security Module (HSM) / Cloud KMS Key
             │
             ▼
   Validated Signature (Zero SmartScreen Warnings)
```

---

## 4. macOS Packaging, Signing & Notarization

Official macOS Dioxus desktop distribution requires native macOS runners and Apple Developer Program credentials.

### Build & Packaging Workflow
1. **`.app` Bundle Structure:**
   ```text
   SiarMessenger.app/
   ├── Contents/
   │   ├── Info.plist
   │   ├── MacOS/
   │   │   └── siar-messenger
   │   ├── Resources/
   │   │   └── AppIcon.icns
   │   └── _CodeSignature/
   ```
2. **Hardened Runtime & Entitlements (`entitlements.plist`):**
   - Enables camera and microphone access with explicit user descriptions:
     ```xml
     <dict>
         <key>com.apple.security.device.camera</key>
         <true/>
         <key>com.apple.security.device.microphone</key>
         <true/>
         <key>com.apple.security.network.client</key>
         <true/>
         <key>com.apple.security.network.server</key>
         <true/>
     </dict>
     ```
3. **Developer ID Code Signing:**
   ```bash
   codesign --deep --force --options runtime \
     --sign "Developer ID Application: Organization Name (TEAM_ID)" \
     --entitlements entitlements.plist \
     SiarMessenger.app
   ```
4. **Apple Notarization with `notarytool`:**
   ```bash
   xcrun notarytool submit SiarMessenger.dmg \
     --keychain-profile "AC_NOTARY" \
     --wait
   ```
5. **Stapling:**
   ```bash
   xcrun stapler staple SiarMessenger.dmg
   ```
   Validates Gatekeeper execution without network lookup.

---

## 5. Release Channels & Versioning

### Release Channels
- **Nightly (`messenger-nightly`):** Automated daily master builds for early testers.
- **Beta (`messenger-beta`):** Feature-complete builds undergoing regression and stress testing.
- **Stable (`messenger`):** Cryptographically signed production releases.

### SemVer & Git Tagging
- Strict Semantic Versioning: `vMAJOR.MINOR.PATCH` (e.g., `v1.0.0`).
- Every stable release is built strictly from an annotated, signed Git tag in CI.
- No production releases are compiled from uncommitted or dirty working trees.

---

## 6. Release Integrity, Checksums & SBOM

### Checksums & Signatures
Every release publishes a signed hash manifest:
```text
SHA256SUMS:
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  SiarMessenger-1.0.0-x86_64.AppImage
a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e  SiarMessengerSetup-1.0.0.exe
f4c6e9d6d33cf85f7c326880df9b3e109d94943f6797a7e4e1a0df96024987f1  SiarMessenger-1.0.0.dmg

SHA256SUMS.sig (GPG Detached Signature)
```

### Software Bill of Materials (SBOM)
- Generated during CI using CycloneDX / SPDX:
  - Complete Rust crate graph and versions.
  - Native C/C++ dependencies (`dav1d`, `rav1e`, `sqlite3`, `opus`).
  - License audit and vulnerability check (`cargo deny`, `cargo audit`).

---

## 7. Desktop Automatic Updater Architecture

Desktop clients feature a secure, signature-verified auto-updater:

```text
Client Application
        │
        ▼ (Periodically or on startup)
Fetch Signed Release Manifest: `https://releases.siar.app/latest-stable.json`
        │
        ▼
Verify Ed25519 / Minisign Signature with Hardcoded Public Key
        │
        ├── Version <= Current: No update needed
        └── Version > Current:
                │
                ▼
        Download Platform-Specific Package
                │
                ▼
        Verify SHA-256 Checksum against Signed Manifest
                │
                ▼
        Stage Update & Prompt User to Restart
                │
                ▼
        Atomic Binary Replacement on Application Restart
```

---

## 8. Release Verification Gates

Before tagging a stable `1.0.0` release, the release pipeline enforces the following gates:
1. **Clean-Machine Smoke Testing:** Validate packages on fresh, unconfigured VMs (Ubuntu, Fedora, Windows 11, clean macOS with Gatekeeper).
2. **Database Migration Verification:** Test automated forward migration from all previous schema versions without data loss (see [`sys-arch/04`](file:///home/irshad/Projects/siar/sys-arch/04-offline-event-log-architecture.md)).
3. **Supply Chain Audit:** Zero high/critical advisories in `cargo audit` and strict license compliance in `cargo deny`.
4. **Signature & Checksum Validation:** Automated CI step downloads uploaded public artifacts and verifies all signatures before publishing the release.