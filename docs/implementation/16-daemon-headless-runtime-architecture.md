# Part 16 — Daemon & Headless Runtime Architecture

Source spec: `sys-arch/16-daemon-headless-runtime-architecture.md` — 211 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**🟡 Substantially complete across multiple major production rounds.**
Includes `siar-runtime` (159 tests), `apps/daemon` (31 tests), `siar-admin` (20 tests), `siar-app-core` (68 tests), and `siar-ui-sync` (29 + 5 tests).
Clippy `-D warnings` and fmt clean. Live black-box verified across multi-node loopback, CLI interactions, and real desktop displays.

## Implementing crate(s)

- `siar-runtime` — lifecycle states, supervisor & circuit breaker, health monitors, command idempotency, IPC socket server, systemd sd_notify, strict TOML configuration, metrics, watchdog, instance lock & graceful handoff, durable audit logging, diagnostic bundles, panic hooks, and quiesced backup coordination.
- `apps/daemon` (`siar-daemon`) — daemon binary with `run`, `ctl`, `unit`, `service-run`, `backup`, `restore`, `audit-verify`, and remote `admin` commands.
- `siar-admin` — remote administration service over authenticated Iroh QUIC endpoint (`comm-admin/1`) with role-based access control (Viewer, Operator, SecurityAdmin, Owner).
- `siar-app-core` — headless application core extracted from UI dependencies, managing messaging, contacts, groups, linking, and storage engines.
- `siar-ui-sync` — domain-level snapshot + delta synchronization protocol connecting detached UI windows to the running daemon.
- `apps/cli` (`siar`) — lightweight CLI client communicating over local IPC socket to the daemon.

---

## Architecture & Subsystems

### 1. Daemon-Centric Architecture (2026-11-15)
- `siar-daemon` is the sole process that opens the database, private cryptographic keys, MLS state, and network transports.
- The Slint desktop window (`apps/desktop`) and CLI client (`apps/cli`) are thin, unprivileged clients communicating via local IPC socket.
- Embedded-core desktop model removed in favor of strict process isolation.

### 2. Runtime Core & Lifecycle (§13-58, §85-110, §167-179)
- **Lifecycle & State Barrier**: §32-36, 111-112 lifecycle phases (Starting → Ready → Maintenance / Degraded → Draining → Stopped).
- **Supervisor & Breakers**: §29-31, 97-98, 121, 142 supervisor with bounded restart policies (5 restarts / 60s, exponential backoff) and circuit breakers.
- **Health & Idempotency**: §89-90, 140-141 health checks; §56-58 command idempotency via deduplicating command ledgers.
- **IPC Server**: §13-18, 144-146, 179 Unix domain sockets and Windows named pipes via `interprocess` 2.4 with abort-safe socket cleanup and peer credentials checking.
- **Service Integration**: §37-39, 99 systemd `sd_notify` integration (with `WATCHDOG_PID` support); `windows-service` integration for Windows SCM.
- **Configuration & SIGHUP**: §85-88 strict TOML configuration; §87 SIGHUP hot-reload for classified dynamic fields; `overlay_config` support.
- **Graceful Handoff**: §100-101, 106-110, 167-168 instance file, upgrade lock, and zero-downtime handoff that verifies process exit before releasing locks.
- **Crash Reporting & Diagnostics**: §93-96 `diag.rs` structured `LogRecord`, `TraceId`, redacted diagnostic bundles, and panic hooks writing backtraces without leaking sensitive message payloads.

### 3. Headless Provisioning & Role Launcher (§72-76)
- **Node Roles & Feature Sets**: §74-75 `NodeRole` (PersonalDevice, RelayGateway, DtnNode, StorageNode, OrganizationNode, TestNode) mapping to strict compile-time `FeatureSet` data tables.
- **Provisioner**: Cryptographically signed `ProvisionPayload` bound to one-time QR challenge; unprovisioned daemons run IPC + enrolment only; valid enrolment persists state (mode 0600) and triggers exit status 75 for systemd automatic restart into active role.
- **Supervised Role Launcher**: `apps/daemon/src/roles.rs` launches child processes (e.g. `siar-emergency-node`) located strictly next to the daemon binary, with `kill_on_drop` and supervisor restart policies.

### 4. Durable Hash-Chained Audit Log (`siar-runtime::audit_file`, 2026-11-07)
- Persistent tamper-evident audit log in `<data>/audit/audit.log` (mode 0600, dir 0700).
- Hash chain: `chain = blake3(previous_chain || body)`, with contiguous sequence numbers and segment rotation (1 MiB segments, 4 kept).
- Chain continuity maintained across segment rotations. Corruption moves invalid log to `audit.corrupt-<ms>/` and initializes a fresh chain with explicit restart marker.
- Audit directory is excluded from `ctl backup` archives and cannot be wiped or rewound by restoring an old backup.

### 5. Daemon Backup & Restore Coordination (§111-115, 2026-11-05)
- Live backup (`ctl backup <dir>`): Takes DB snapshot via `create_snapshot()`, archives snapshot and non-DB state while skipping live WAL/volumes, encrypted with Argon2id + XChaCha20-Poly1305.
- Two-phase restore (`ctl restore <dir>` in maintenance mode): Decrypts into sibling staging folder; on next start `apply_pending_restore` swaps files before DB opens, preserving prior state in `.pre-restore-<ms>`.
- Rollback safety: Injected failure tests verify atomic rollback to staging if placement fails.

### 6. Remote Administration (`siar-admin`, §66-71, §159-160, §182)
- Dedicated Iroh QUIC endpoint with independent key (`<data>/admin/endpoint.key`, mode 0600) and ALPN `comm-admin/1`.
- Strictly allow-listed Ed25519 public keys enforced during handshake before any bytes are parsed.
- RBAC roles: `Viewer < Operator < SecurityAdmin < Owner`.
- Authenticated, authorized, audited, and rate-limited. No shell access and no message content visibility.
- Tooling: `siar-daemon admin-keygen` and `siar-daemon admin --key-file <key> <node-id> <op>`.

### 7. Window ⇄ Daemon State Synchronization (`siar-ui-sync`)
- Connects Slint GUI window to detached daemon over IPC socket.
- Protocol carries domain wire sections: Core, Identity, Conversations, Contacts, Network, Power, Timeline, Groups, Invites, SecurityEvents, Devices, Diagnostics, Previews, Link.
- Idempotent snapshots and bounded deltas; wire commands re-validated on the daemon.
- Supports security events sync, event resolution, daemon desktop notifications (`notify-rust`), and pairwise session initialization.

### 8. Relay Mailbox & Scheduled Collection (2026-11-15)
- Anonymous deposit: `send-anon` stores message, deposits with relay, and marks `Sent` (retrying with exponential backoff).
- Anonymous polling: `check-mailbox-anon` issues signed check-in and receives token deliveries.
- Background scheduled collection: polls configured relays for rotating contact slices (32 per poll), fully power-gated via `siar-power`.

---

## Known Gaps / Open Items

- **GUI Snapshot Serialization**: UI types still rely on domain DTOs; full domain snapshot/delta with dirty tracking across thousands of conversations is a future optimization.
- **Windows Named Pipes**: Compiled with `interprocess` but requires validation on physical Windows hosts.
- **Mobile Daemon Hosting**: Android foreground service hosting and iOS background execution windows (§42-46) remain to be implemented.
- **Multi-Account in One Daemon**: Directory namespace isolation exists (§78, 81-84); account-scoped command ledgers inside a single daemon process are not yet built.
- **Plugin Host Isolation**: §120 process isolation for third-party plugins.
- **Unix FD Passing**: §63 socket FD passing.
- **Remote Restore Restart**: Remote restore currently stages files and requires manual/service restart to execute swap.
