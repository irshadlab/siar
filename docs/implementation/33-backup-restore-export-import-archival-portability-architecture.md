# Part 33 — Backup, Restore, Export/Import, Archival & Long-Term Data Portability Architecture

Source spec: `sys-arch/33-backup-restore-export-import-archival-portability-architecture.md` — 280 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**🟡 Substantially complete (Offline & Live Daemon Backup/Restore Subsystem implemented).**
Implemented in `crates/siar-backup` and integrated into `siar-desktop` and `siar-daemon` (`ctl backup` / `ctl restore`). 20 unit tests in `siar-backup` + 31 in `siar-daemon`. Verified with disaster-recovery end-to-end tests and fault-injection rollback testing.

## Implementing crate(s)

- `siar-backup` — encrypted archive packaging, cryptographic integrity checking, format validation, and atomic staging.
- `apps/desktop` — offline backup/restore commands (`siar-desktop backup`, `restore`, `backup-info`).
- `apps/daemon` (`siar-daemon`) — live quiesced backup (`ctl backup`) and staged restore (`ctl restore`) with atomic pre-startup application.

---

## Architecture & Features

### 1. Cryptographic Archive Format
- **Encryption & Key Derivation**: Argon2id password-based key derivation + XChaCha20-Poly1305 authenticated encryption.
- **Header AAD Binding**: Archive header is cryptographically bound as Additional Authenticated Data (AAD); any modified or flipped bit anywhere in header or payload is immediately detected before any file operations occur.
- **Hostile Archive Defenses**: Path traversal (`..`), absolute paths, duplicate entries, decompressed size explosions, and invalid KDF parameters are strictly rejected during archive inspection.

### 2. Concurrency & Instance Locking
- Single-instance lock file (`siar.lock`) prevents multiple processes from opening or mutating the data folder concurrently.
- Offline backup/restore commands refuse execution if another instance is active.

### 3. Live Daemon Backup (2026-11-05)
- Performed while the daemon is actively serving clients via `ctl backup <abs-dir>`.
- Takes a snapshot via stoolap `create_snapshot()`.
- Archives snapshot directories and non-DB metadata files; live WAL and volume files are explicitly filtered out before opening, ensuring half-checkpointed transactions cannot corrupt archives.
- Backup passphrase provided via secure file reference or environment variable (`secrets.backup_passphrase`), never plain text configuration.

### 4. Two-Phase Safe Restore & Atomic Rollback
- Executed in daemon maintenance mode (`ctl maintenance on`).
- Decrypts and validates archive contents into an isolated sibling staging directory (`.staging-restore-<ms>`).
- On subsequent daemon startup, `apply_pending_restore` atomically swaps the staged files into place before the database or network sockets open.
- Previous data folder is safely preserved in `<data>.pre-restore-<ms>`.
- Database rebuilt via `restore_snapshot`.
- Fault injection tests verify that failures during placement or DB restoration automatically roll back files to staging, leaving the existing installation undamaged.

### 5. Multi-Device Linking Coverage
- Archives preserve identity credentials: `root.key`, `linked_devices.bin`, `joined_account.bin`, and `account_id.txt`.
- Restores files with strict owner-only permissions (mode 0600, directories 0700).

### 6. Audit Log Protection
- The audit log directory (`<data>/audit/`) is explicitly excluded from backup archives.
- Restoring an archive never rewinds or deletes the active audit log chain, ensuring an auditable record of the restoration itself is maintained.

---

## Known Gaps / Open Items

- Graphical UI backup and restore wizard in the desktop client (currently exposed via CLI and daemon `ctl`).
- Scheduled automated background backup jobs.
- Selective data export / import (e.g. exporting a single conversation or contact vCard).
