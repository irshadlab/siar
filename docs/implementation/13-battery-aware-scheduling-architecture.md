# Part 13 — Battery-Aware Scheduling Architecture

Source spec: `sys-arch/13-battery-aware-scheduling-architecture.md` — 145 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**🟡 Substantially complete: `siar-power` crate implemented** (122 unit + 10 scenario tests; clippy `-D warnings` clean).
Policy engine, budget trackers, deferral scheduler, wake batching, and discovery duty-cycling are fully built and integrated with core workspace crates.

## Implementing crate(s)

- `siar-power` — state tracking, layered policy engine, durable-deferral scheduler, wake batching, discovery duty cycle, vendor quirks, host power reports, extension governor, and sysfs power source.
- `siar-routing-policy`, `siar-crash-recovery`, `siar-resource-limits` — consuming `BatteryLevelClass`, `ThermalState`, and `ModeFlags::from_power`.
- `apps/emergency-node` — relay forwarding pauses on low battery while bundle storage never pauses.
- `siar-app-core::power_gate` — attachment previews and history preload are gated; message traffic is never gated.

## Completed Sections and Architecture

- **State & Budgets**:
  - §4-8, 15: Power and battery state modeling.
  - §9-14, 69-70: Power profiles, configurations, and user preferences.
  - §16-19, 140: Power budgets and allocations across subsystems.
- **Layered Policy Engine & Deferral Scheduler**:
  - §20-28, 68, 72-74, 96-97: Layered policy engine evaluating power and thermal states against subsystem requests.
  - §21, 75-80, 110-112: Durable-deferral scheduler (aging, thundering-herd control, restart-safe persistence).
  - §30-32: Wake batching and coalesced timer triggers.
  - §34-38, 91: Discovery duty cycle and radio scan throttling under battery pressure.
  - §39-57, 138: Power advice for files, DTN transfers, and media sessions.
- **Signals, Hysteresis & Diagnostics**:
  - §81-84: State hysteresis, coalescing, and epoch numbering.
  - §85, 115: Power metrics and structured diagnostic lines.
  - §86-87: Privacy classes and peer power hints.
- **Host & Hardware Integration**:
  - §100-102: Hardware/vendor quirks data table (conservative defaults; tested against "must/required" clauses).
  - §105: `HostReport` stable host API for platform power telemetry.
  - §106-108: `ExtensionGovernor` (clamped claims, bounded queues, CPU-scaled credit bucket, strike/backoff suspension).
  - §113: Recovery ordering after power restoration.
  - §114: Semantic event outbox (fires on meaningful threshold transitions, not every millivolt).
  - §116-119: User feedback copy, emergency banner, thermal call notice, `NotificationGate`.
  - §120-129, §133: Scenario tests covering complex battery transitions.
  - §139: Linux sysfs battery reader and push event source (`PushPowerSource`).

## Policy Bugs Caught and Fixed by Tests

- **Reserve domain starvation**: Reserve-domain operations (delivery receipts, identity updates) were inadvertently blocked at critical battery; corrected so critical identity and receipts continue.
- **Eager transfer deferral**: Heavy file transfers were being deferred prematurely even at *High* battery; corrected threshold mapping.

## Known Gaps / Open Items

- **Mobile platform telemetry**: Android `BatteryManager`/Doze/foreground-service producers and iOS power integration (§58-64) — the Rust interface is complete (`HostReport`/`PushPowerSource`), but Kotlin/Swift producers remain to be written.
- **Hardware tuning**: Empirical parameter tuning (§99) and real-device/benchmark suites (§130-136) require physical device test benches.
- **Consumer coverage**: Desktop bulk-file transfers do not exist yet, so §39-49 limits have no desktop consumer beyond image attachment previews.
- **UI consumption**: Semantic event outbox (§114) and user notices (§116) are not yet wired into the desktop GUI presentation layer.
