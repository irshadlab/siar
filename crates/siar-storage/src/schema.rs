//! Schema (plan.md §20). Phase 1 subset: just `messages` and `outbox` —
//! receipts/sessions arrive with the crates that actually need them
//! (Phase 2+) rather than being pre-declared unused. `contacts` (see
//! this file's own section below) landed with the desktop app's
//! persistent contact book.

use crate::StorageError;
use stoolap::Database;

pub(crate) fn apply(db: &Database) -> Result<(), StorageError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS messages (
            message_id      TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            sender_device   TEXT NOT NULL,
            seq_num         INTEGER NOT NULL,
            timestamp_millis INTEGER NOT NULL,
            delivery_state  TEXT NOT NULL,
            payload         TEXT NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_id
         ON messages (message_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // Plan.md §20: messages(conversation_id, seq_num) is the timeline
    // query's index — every conversation-open call hits this.
    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_messages_conversation_sequence
         ON messages (conversation_id, seq_num)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // Plan.md §17: the outbox is the transactional-delivery mechanism.
    // `next_attempt_at` drives the retry scheduler's polling query.
    // Denormalized recipient info (Phase-2 stand-in — plan.md §20's
    // `contacts`/`conversation_members` tables don't exist yet, so
    // the retry scheduler needs *something* to resend to. Once
    // those tables land, this becomes a conversation_id lookup
    // instead and this column goes away).
    db.execute(
        "CREATE TABLE IF NOT EXISTS outbox (
            message_id      TEXT NOT NULL,
            next_attempt_at INTEGER NOT NULL,
            attempts        INTEGER NOT NULL DEFAULT 0,
            peer_ticket_hex TEXT NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_message_id
         ON outbox (message_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_outbox_next_attempt
         ON outbox (next_attempt_at)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // plan.md §81: blob garbage collection is reference-counted — a blob
    // is only deleted once nothing (a message, a thumbnail, a draft)
    // references it anymore. Phase 4 scope: the count exists and is
    // incremented/decremented; the actual GC sweep that acts on
    // `ref_count = 0` is future work once there's more than one
    // reference source to make GC worth running.
    db.execute(
        "CREATE TABLE IF NOT EXISTS blobs (
            blob_hash TEXT NOT NULL,
            ciphertext TEXT NOT NULL,
            ref_count INTEGER NOT NULL DEFAULT 1
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_blobs_hash
         ON blobs (blob_hash)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // plan.md §20's group tables — see group_repo.rs. `group_state` is
    // one row per conversation (just the epoch counter); membership is
    // normalized out into `group_members` rather than a serialized blob
    // column so a single member's role/join-epoch can be queried without
    // deserializing the whole group.
    db.execute(
        "CREATE TABLE IF NOT EXISTS group_state (
            conversation_id TEXT NOT NULL,
            epoch           INTEGER NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_group_state_conversation
         ON group_state (conversation_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS group_members (
            conversation_id  TEXT NOT NULL,
            account_id       TEXT NOT NULL,
            role             TEXT NOT NULL,
            joined_at_epoch  INTEGER NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_group_members_conversation
         ON group_members (conversation_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // plan.md §20's "conversations/contacts" tables, added for the
    // desktop app's persistent contact book (see contact_repo.rs). One
    // row per known peer *device* — `account_id` is duplicated across a
    // peer's multiple devices on purpose, same denormalization
    // `group_members` already uses, rather than a separate `accounts`
    // table this pass doesn't need yet. `key_package_b64` is nullable:
    // a 1:1-only contact (never added to a group) may never have had
    // one recorded.
    db.execute(
        "CREATE TABLE IF NOT EXISTS contacts (
            device_id        TEXT NOT NULL,
            account_id       TEXT NOT NULL,
            display_name     TEXT NOT NULL,
            ticket_text      TEXT NOT NULL,
            key_package_b64  TEXT,
            added_at_millis  INTEGER NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_contacts_device
         ON contacts (device_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_contacts_account
         ON contacts (account_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // Security log (ui-ux-15 §43): events survive a restart so a resolved
    // warning stays resolved and a standing one isn't re-raised at launch.
    db.execute(
        "CREATE TABLE IF NOT EXISTS security_events (
            event_id           INTEGER NOT NULL,
            kind               TEXT NOT NULL,
            occurred_at_millis INTEGER NOT NULL,
            resolved           INTEGER NOT NULL DEFAULT 0,
            related_device     TEXT,
            related_account    TEXT,
            actions            TEXT NOT NULL DEFAULT ''
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_security_events_id
         ON security_events (event_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    // Group members' addresses learned from invites and commits, so they
    // stay reachable across restarts without being saved contacts.
    db.execute(
        "CREATE TABLE IF NOT EXISTS peer_directory (
            account_id  TEXT NOT NULL,
            device_id   TEXT NOT NULL,
            ticket_text TEXT NOT NULL
        )",
        (),
    )
    .map_err(StorageError::from_stoolap)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_directory_device
         ON peer_directory (account_id, device_id)",
        (),
    )
    .map_err(StorageError::from_stoolap)?;

    Ok(())
}
