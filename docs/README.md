# siar implementation docs

One file per `sys-arch/` spec, **same filename**, so the two trees line up 1:1. ROADMAP.md (repo root) stays the entry point for *priority/what's-next*; each file here is the detailed *what's actually written* record for that one spec — crate(s), section coverage, and named gaps.

Generated 2026-09-01 from ROADMAP.md + project notes. Files marked ⚪ below are placeholders (section count only, no reconciliation done yet) — filling those in, spec by spec, is the real remaining work this directory exists to track.

| Spec file | Sections | Status |
|---|---|---|
| [01-protocol-extension-system-architecture.md](01-protocol-extension-system-architecture.md) | 108/108 | 108/108 (100%) |
| [02-multi-device-identity-architecture.md](02-multi-device-identity-architecture.md) | 204/204 | 204/204 (100%) |
| [03-transport-routing-policy-engine-architecture.md](03-transport-routing-policy-engine-architecture.md) | 200/200 | 200/200 (100%) |
| [04-offline-event-log-architecture.md](04-offline-event-log-architecture.md) | 10/95 | 10/95 (11%) |
| [05-robust-file-blob-subsystem-architecture.md](05-robust-file-blob-subsystem-architecture.md) | 23/210 | 23/210 (11%) |
| [06-dtn-store-carry-forward-architecture.md](06-dtn-store-carry-forward-architecture.md) | 50/192 | 50/192 (26%) |
| [07-capability-negotiation-architecture.md](07-capability-negotiation-architecture.md) | 19/164 | 19/164 (12%) |
| [08-resource-limits-backpressure-architecture.md](08-resource-limits-backpressure-architecture.md) | 56/193 | 56/193 (29%) |
| [09-crash-recovery-architecture.md](09-crash-recovery-architecture.md) | 15/186 | 15/186 (8%) |
| [10-fuzzing-protocol-test-suite-architecture.md](10-fuzzing-protocol-test-suite-architecture.md) | 0/207 | 0/207 (not started) |
| [11-relay-self-hosted-infrastructure-architecture.md](11-relay-self-hosted-infrastructure-architecture.md) | 0/194 | 0/194 (not started) |
| [12-multipath-networking-architecture(1).md](<12-multipath-networking-architecture(1).md>) | 0/178 | 0/178 (not started) |
| [13-battery-aware-scheduling-architecture.md](13-battery-aware-scheduling-architecture.md) | 0/145 | 0/145 (not started) |
| [14-proximity-abstraction-architecture.md](14-proximity-abstraction-architecture.md) | 0/131 | 0/131 (not started) |
| [15-qr-nfc-bootstrap-pairing-architecture.md](15-qr-nfc-bootstrap-pairing-architecture.md) | 0/176 | 0/176 (not started) |
| [16-daemon-headless-runtime-architecture.md](16-daemon-headless-runtime-architecture.md) | 0/211 | 0/211 (not started) |
| [17-emergency-priority-classes-architecture.md](17-emergency-priority-classes-architecture.md) | 0/188 | 0/188 (not started) |
| [18-network-diagnostics-path-visualization-architecture.md](18-network-diagnostics-path-visualization-architecture.md) | 0/206 | 0/206 (not started) |
| [19-c-abi-ffi-architecture.md](19-c-abi-ffi-architecture.md) | 0/170 | 0/170 (not started) |
| [20-embedded-linux-node-architecture.md](20-embedded-linux-node-architecture.md) | 0/230 | 0/230 (not started) |
| [21-third-party-protocol-extensions-architecture.md](21-third-party-protocol-extensions-architecture.md) | 0/248 | 0/248 (not started) |
| [22-wasm-compatible-components-architecture.md](22-wasm-compatible-components-architecture.md) | 0/254 | 0/254 (not started) |
| [23-external-interoperability-suite-architecture.md](23-external-interoperability-suite-architecture.md) | 0/255 | 0/255 (not started) |
| [24-plugin-module-ecosystem-architecture.md](24-plugin-module-ecosystem-architecture.md) | 0/305 | 0/305 (not started) |
| [25-android-direct-hardware-surface-zero-copy-media-architecture.md](25-android-direct-hardware-surface-zero-copy-media-architecture.md) | 0/213 | 0/213 (not started) |
| [26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md](26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md) | 0/220 | 0/220 (not started) |
| [27-rust-driven-android-native-build-packaging-automation.md](27-rust-driven-android-native-build-packaging-automation.md) | 0/279 | 0/279 (not started) |
| [28-production-security-e2ee-key-management-privacy-architecture.md](28-production-security-e2ee-key-management-privacy-architecture.md) | 46/127 | 46/127 (36%) |
| [29-realtime-calls-media-session-protocol-architecture.md](29-realtime-calls-media-session-protocol-architecture.md) | 0/275 | 0/275 (not started) |
| [30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md](30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md) | 0/269 | 0/269 (not started) |
| [31-notifications-push-background-delivery-lifecycle-architecture.md](31-notifications-push-background-delivery-lifecycle-architecture.md) | 0/308 | 0/308 (not started) |
| [32-search-indexing-local-knowledge-privacy-architecture.md](32-search-indexing-local-knowledge-privacy-architecture.md) | 0/242 | 0/242 (not started) |
| [33-backup-restore-export-import-archival-portability-architecture.md](33-backup-restore-export-import-archival-portability-architecture.md) | 0/280 | 0/280 (not started) |
| [ui-ux-01-product-foundation-cross-platform-interaction-architecture.md](ui-ux-01-product-foundation-cross-platform-interaction-architecture.md) | 0/83 | 0/83 (not started) |
| [ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md](ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md) | 0/227 | 0/227 (not started) |
| [ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md](ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md) | 0/217 | 0/217 (not started) |
| [ui-ux-04-conversation-list-inbox-architecture.md](ui-ux-04-conversation-list-inbox-architecture.md) | 0/213 | 0/213 (not started) |
| [ui-ux-05-conversation-message-timeline-architecture.md](ui-ux-05-conversation-message-timeline-architecture.md) | 0/254 | 0/254 (not started) |
| [ui-ux-06-message-composer-attachments-voice-notes-drafts-architecture.md](ui-ux-06-message-composer-attachments-voice-notes-drafts-architecture.md) | 0/309 | 0/309 (not started) |
| [ui-ux-07-calls-realtime-media-architecture.md](ui-ux-07-calls-realtime-media-architecture.md) | 0/235 | 0/235 (not started) |
| [ui-ux-08-contacts-requests-verification-identity-architecture.md](ui-ux-08-contacts-requests-verification-identity-architecture.md) | 0/227 | 0/227 (not started) |
| [ui-ux-09-groups-membership-roles-architecture.md](ui-ux-09-groups-membership-roles-architecture.md) | 0/224 | 0/224 (not started) |
| [ui-ux-10-files-media-gallery-transfer-architecture.md](ui-ux-10-files-media-gallery-transfer-architecture.md) | 0/161 | 0/161 (not started) |
| [ui-ux-11-search-local-knowledge-retrieval-architecture.md](ui-ux-11-search-local-knowledge-retrieval-architecture.md) | 0/148 | 0/148 (not started) |
| [ui-ux-12-nearby-qr-nfc-pairing-device-linking-architecture.md](ui-ux-12-nearby-qr-nfc-pairing-device-linking-architecture.md) | 0/237 | 0/237 (not started) |
| [ui-ux-13-notifications-background-incoming-call-architecture.md](ui-ux-13-notifications-background-incoming-call-architecture.md) | 0/215 | 0/215 (not started) |
| [ui-ux-14-presence-typing-receipts-status-architecture.md](ui-ux-14-presence-typing-receipts-status-architecture.md) | 0/197 | 0/197 (not started) |
| [ui-ux-15-security-center-devices-keys-recovery-architecture.md](ui-ux-15-security-center-devices-keys-recovery-architecture.md) | 184/221 | 184/221 (83%) |
| [ui-ux-16-backup-restore-export-migration-architecture.md](ui-ux-16-backup-restore-export-migration-architecture.md) | 0/223 | 0/223 (not started) |
| [ui-ux-17-emergency-sos-offline-mesh-architecture.md](ui-ux-17-emergency-sos-offline-mesh-architecture.md) | 0/240 | 0/240 (not started) |
| [ui-ux-18-settings-privacy-notifications-data-controls-architecture.md](ui-ux-18-settings-privacy-notifications-data-controls-architecture.md) | 0/219 | 0/219 (not started) |
| [ui-ux-19-plugin-module-ecosystem-architecture.md](ui-ux-19-plugin-module-ecosystem-architecture.md) | 0/266 | 0/266 (not started) |
| [ui-ux-20-diagnostics-network-paths-advanced-developer-architecture.md](ui-ux-20-diagnostics-network-paths-advanced-developer-architecture.md) | 0/225 | 0/225 (not started) |
| [ui-ux-21-accessibility-inclusive-interaction-architecture (1).md](<ui-ux-21-accessibility-inclusive-interaction-architecture (1).md>) | 0/294 | 0/294 (not started) |
| [ui-ux-22-design-system-tokens-typography-icons-motion-architecture.md](ui-ux-22-design-system-tokens-typography-icons-motion-architecture.md) | 0/253 | 0/253 (not started) |
| [ui-ux-23-responsive-adaptive-desktop-tablet-foldable-phone-layout-architecture.md](ui-ux-23-responsive-adaptive-desktop-tablet-foldable-phone-layout-architecture.md) | 0/223 | 0/223 (not started) |
| [ui-ux-24-error-loading-empty-offline-degraded-state-architecture.md](ui-ux-24-error-loading-empty-offline-degraded-state-architecture.md) | 0/205 | 0/205 (not started) |
| [ui-ux-25-onboarding-first-run-permission-education-architecture.md](ui-ux-25-onboarding-first-run-permission-education-architecture.md) | 0/209 | 0/209 (not started) |
| [ui-ux-26-performance-virtualization-large-data-ui-architecture.md](ui-ux-26-performance-virtualization-large-data-ui-architecture.md) | 0/276 | 0/276 (not started) |
| [ui-ux-27-ui-testing-screenshot-interaction-release-quality-gates-architecture.md](ui-ux-27-ui-testing-screenshot-interaction-release-quality-gates-architecture.md) | 0/274 | 0/274 (not started) |

---

## Supplementary Architecture & Operational Guides

In addition to the per-spec implementation tracking documents above, the following comprehensive guides and architectural blueprints are maintained in this directory:

- [plan.md](plan.md) — Master System Integration Architecture & Phased Implementation Roadmap
- [nix-installation-guide.md](nix-installation-guide.md) — Nix Installation & Multi-Distro Configuration Guide
- [mobile-driven-ui-ux-architecture.md](mobile-driven-ui-ux-architecture.md) — Mobile-Driven UI/UX System & Architecture (Dioxus + Rust Mobile Core + Android Kotlin Bridge)
- [ui-gui-architecture.md](ui-gui-architecture.md) — Universal & Desktop Dioxus UI/GUI Architecture
- [off-grid.md](off-grid.md) — Off-Grid Communications & Emergency Field Deployment Guide
- [video-codec.md](video-codec.md) — Video Codec Pipeline, Hardware Acceleration & AV1 Zero-Copy Architecture
- [images.md](images.md) — System Architecture Visual Diagrams and Assets
