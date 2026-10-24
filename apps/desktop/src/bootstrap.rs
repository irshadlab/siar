//! Startup: identity, database, transport and services.
//!
//! Unchanged in substance from the Dioxus shell — the UI toolkit swap
//! does not touch how the core comes up. One addition: `messages` (read
//! access to stored messages, for real delivery ticks).
//! This module and `core.rs` are the only places in the desktop app that
//! touch `siar-messaging` / `siar-transport`; the UI crate cannot (it
//! does not depend on them).

use anyhow::{Context, Result};
use siar_crypto::DeviceIdentity;
use siar_domain::{AccountId, DeviceId};
use siar_messaging::{
    GroupService, InMemoryDeviceDirectory, InMemoryKeyPackageDirectory, KeyPackageDirectory,
    MemberDevice, MessageService, PeerTicket,
};
use siar_storage::{ContactRepository, MessageRepository, StoolapContactRepository, StoredContact};
use siar_transport::SiarEndpoint;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Where this device's identity/account id/database live on disk —
/// closes a real gap flagged across earlier sessions: both `apps/cli`
/// and this app previously regenerated a fresh `DeviceIdentity` and
/// `AccountId` and opened an in-memory database on *every* launch, so
/// nothing — not a paired peer, not a saved contact, not this device's
/// own identity — ever survived a restart. `directories::ProjectDirs`
/// resolves the OS-appropriate data directory (XDG on Linux, Known
/// Folder API on Windows, Standard Directory on macOS) rather than
/// hand-rolling `$HOME/.local/share` parsing, which would silently
/// ignore `XDG_DATA_HOME` and break outright on Windows.
pub struct DataPaths {
    pub data_dir: std::path::PathBuf,
    identity: std::path::PathBuf,
    account_id: std::path::PathBuf,
    device_id: std::path::PathBuf,
    database: std::path::PathBuf,
}

pub fn resolve_data_paths() -> Result<DataPaths> {
    let dirs = directories::ProjectDirs::from("dev", "irshad", "siar")
        .context("couldn't resolve a data directory for this platform")?;
    let data_dir = dirs.data_dir();
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    Ok(DataPaths {
        data_dir: data_dir.to_path_buf(),
        identity: data_dir.join("identity.bin"),
        account_id: data_dir.join("account_id.txt"),
        device_id: data_dir.join("device_id.txt"),
        database: data_dir.join("siar.db"),
    })
}

/// Loads an id previously written by `write_id_file`, or generates and
/// persists a fresh one on first run. A bare UUID string, not
/// postcard/JSON — this file has exactly one field, and a plain-text
/// UUID is trivially inspectable (`cat account_id.txt`) for anyone
/// debugging a broken install, which a serialized envelope wouldn't be.
fn load_or_create_id<T>(
    path: &std::path::Path,
    from_uuid: impl Fn(uuid::Uuid) -> T,
    to_uuid: impl Fn(&T) -> uuid::Uuid,
    generate: impl Fn() -> T,
) -> Result<T> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let id = uuid::Uuid::parse_str(text.trim())
            .with_context(|| format!("{} did not contain a valid UUID", path.display()))?;
        Ok(from_uuid(id))
    } else {
        let id = generate();
        std::fs::write(path, to_uuid(&id).to_string())
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(id)
    }
}

/// Everything `bootstrap_messaging` assembles — a small struct instead
/// of an ever-growing tuple, same reasoning and same shape as
/// `apps/cli`'s own `Bootstrapped` (that file's struct doc comment
/// explains the "why a struct" choice; not repeated here).
pub struct Bootstrapped {
    pub service: Arc<MessageService>,
    /// Drives every MLS group action from `command_loop` (create, add
    /// member, send, join) and routes incoming group frames from
    /// `incoming_loop` (see that function's own doc comment) — the
    /// group UI's whole backend surface.
    pub group_service: Arc<GroupService>,
    pub my_ticket: PeerTicket,
    pub incoming_rx: mpsc::Receiver<siar_transport::IncomingFrame>,
    /// This device's own identifiers — the group UI needs
    /// `local_account` for `CreateGroup`'s `founder` field
    /// (`GroupService::create_group_mls` takes it as a parameter, it
    /// isn't implicit), and `device_id` for the "publish key package"
    /// action's printed output (mirrors `apps/cli`'s
    /// `publish_key_package` command exactly — see that function's own
    /// doc comment for why the device id/account id/ticket all need to
    /// travel with the key package bytes).
    pub local_account: AccountId,
    pub device_id: DeviceId,
    /// Was constructed but discarded (`_key_package_directory`) before
    /// the group UI existed — nothing consumed it. Now needed for the
    /// "publish key package" action, same in-memory-only directory
    /// `apps/cli` uses (see `key_package_directory.rs`'s own doc
    /// comment on scope: this is a single-process placeholder, not
    /// next.md §41's real distribution system).
    ///
    /// Kept alive here (not just as a local in `bootstrap()`) for a
    /// future "republish key package" UI action — `apps/cli` exposes
    /// that as its own on-demand `publish-key-package` subcommand;
    /// this desktop build only ever calls `publish_key_package` once,
    /// eagerly, at startup (a few lines below), so nothing currently
    /// reads this field back out of `Bootstrapped` after construction.
    /// `#[allow(dead_code)]` rather than deleting real, intentionally-
    /// placed infrastructure on the strength of "nothing calls it
    /// yet" — the same reasoning this workspace applies to every
    /// other "computation built, no real caller yet" gap.
    #[allow(dead_code)]
    pub key_package_directory: Arc<InMemoryKeyPackageDirectory>,
    /// This device's own MLS key package, published once at startup
    /// (mirrors `apps/cli`'s `listen --publish-key-package` flow, just
    /// unconditional here since a desktop session doesn't have an
    /// equivalent flag) and immediately base64-encoded for display —
    /// same reasoning as `publish_key_package`'s own doc comment on why
    /// the bytes have to move by copy-paste today. `None` if publishing
    /// failed; the app still starts (1:1 messaging doesn't depend on
    /// this), just without anything to show for "add me to a group."
    pub key_package_b64: Option<String>,
    /// Backs the persistent contact book (`AppCommand::SaveContact`/
    /// `RemoveContact`) — see `contact_repo.rs`'s own doc comment for
    /// exactly what this does and doesn't close.
    pub contact_repo: Arc<dyn ContactRepository + Send + Sync>,
    /// Every contact already on disk, loaded once at startup so
    /// `app.rs` can seed `ContactListState` and re-`register` each one
    /// into `device_directory` without a second async round-trip.
    pub saved_contacts: Vec<StoredContact>,
    /// Needed so `AddGroupMember` can `register` the new member's
    /// device before calling `add_member_mls` — that method looks up
    /// `devices_for(new_member)` to find where to send the welcome
    /// (see its own source: it filters `directory.devices_for
    /// (new_member)` for the matching device), exactly the same
    /// prerequisite `apps/cli`'s `group_add_member` satisfies with
    /// `boot.device_directory.register(...)` before its own
    /// `add_member_mls` call.
    pub device_directory: Arc<InMemoryDeviceDirectory>,
    /// Read access to stored messages, so the UI can show real delivery
    /// ticks (Sent / Delivered) instead of staying on "Saving…".
    pub messages: Arc<dyn MessageRepository + Send + Sync>,
    /// Groups whose MLS state was reopened from disk at start-up.
    pub restored_groups: Vec<siar_domain::ConversationId>,
    /// Per-device data directory (verification marks live here).
    pub data_dir: std::path::PathBuf,
    /// When this device's identity key was first created (file creation
    /// time; falls back to last-modified, then 0 if the platform has neither).
    pub identity_created_at_millis: u64,
}

/// Runs on a background Tokio task, owns the `MessageService`, and is the
/// only place in this binary that touches `siar-messaging`/`siar-transport`
/// directly (plan.md §86's dependency direction, enforced by convention
/// here since Rust's visibility rules alone don't stop a component file
/// from importing `siar_messaging` — discipline plus code review is what
/// actually holds this boundary, same as in the original architecture
/// doc's own C++ analogue).
pub async fn bootstrap_messaging() -> Result<Bootstrapped> {
    let paths = resolve_data_paths()?;

    // Persistent identity/account/device id — first real fix in this
    // pass: previously a fresh `DeviceIdentity`/`AccountId`/`DeviceId`
    // was minted every launch (see `apps/cli`'s identical Phase-3
    // stand-in note this used to carry), which made a persistent
    // contact book pointless — a saved peer's ticket would still be
    // for a device identity that no longer exists by the next launch,
    // and this device's own identity wouldn't be recognizable to
    // anyone who'd saved *it* as a contact either.
    let identity = if paths.identity.exists() {
        DeviceIdentity::load_from_file(&paths.identity).context("loading saved device identity")?
    } else {
        let identity = DeviceIdentity::generate();
        identity
            .save_to_file(&paths.identity)
            .context("saving new device identity")?;
        identity
    };
    let identity_created_at_millis = std::fs::metadata(&paths.identity)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let device_id = load_or_create_id(
        &paths.device_id,
        DeviceId::from_uuid,
        DeviceId::as_uuid,
        DeviceId::new,
    )?;
    let local_account = load_or_create_id(
        &paths.account_id,
        AccountId::from_uuid,
        AccountId::as_uuid,
        AccountId::new,
    )?;

    // plan.md §84: staged startup — local state (identity, DB) comes up
    // before anything touches the network, so the UI shell can render
    // immediately and network bootstrap continues underneath it.
    //
    // Real on-disk database now, not `open_in_memory()` — the other
    // half of the persistence fix above; an in-memory DB would have
    // discarded every message/group/contact row the moment the process
    // exited regardless of how stable the identity above is.
    let db_path = paths.database.display().to_string();
    let db = siar_storage::open(&db_path)
        .with_context(|| format!("opening local database at {db_path}"))?;
    let messages = Arc::new(siar_storage::StoolapMessageRepository::new(db.clone()));
    let message_view: Arc<dyn MessageRepository + Send + Sync> = messages.clone();
    let outbox = Arc::new(siar_storage::StoolapOutboxRepository::new(db.clone()));
    let groups = Arc::new(siar_storage::StoolapGroupRepository::new(db.clone()));
    let contact_repo: Arc<dyn ContactRepository + Send + Sync> =
        Arc::new(StoolapContactRepository::new(db.clone()));
    let blobs: Arc<dyn siar_storage::BlobRepository + Send + Sync> =
        Arc::new(siar_storage::StoolapBlobRepository::new(db));
    let blob_store: Arc<dyn siar_transport::BlobStore> =
        Arc::new(siar_messaging::StorageBlobStore(blobs.clone()));

    let (tx, rx) = mpsc::channel::<siar_transport::IncomingFrame>(64);
    // iroh-base 1.0.3's `SecretKey::generate()` takes no RNG argument —
    // see apps/cli/src/main.rs's identical fix for the full explanation.
    let iroh_secret = iroh::SecretKey::generate();
    let endpoint = Arc::new(SiarEndpoint::bind(iroh_secret, tx, blob_store).await?);

    let my_ticket = PeerTicket {
        endpoint_addr: endpoint.addr(),
        x25519_public: identity.x25519_public().to_bytes(),
        ed25519_verifying: identity.verifying_key().to_bytes(),
    };

    let device_directory = Arc::new(InMemoryDeviceDirectory::new());
    let key_package_directory = Arc::new(InMemoryKeyPackageDirectory::new());

    // Re-populate `device_directory` from every saved contact before
    // anything else touches it — `add_member_mls`'s fanout (and a
    // future 1:1 send to a contact who isn't the single `ActivePeer`)
    // both resolve through this directory, so a contact that was saved
    // last session but never re-registered here would be invisible to
    // both even though its row is sitting right there in `contacts`.
    let saved_contacts = contact_repo.list().context("loading saved contacts")?;
    for contact in &saved_contacts {
        match PeerTicket::decode(&contact.ticket_text) {
            Ok(ticket) => device_directory.register(
                contact.account_id,
                MemberDevice {
                    device_id: contact.device_id,
                    ticket,
                },
            ),
            Err(e) => tracing::warn!(
                error = %e,
                device_id = ?contact.device_id,
                "dropped a saved contact with an unparseable ticket — was the contacts table edited by hand?"
            ),
        }
    }

    // `MessageService` and `GroupService` both take a `DeviceIdentity`
    // by value, and both represent this same local device —
    // `DeviceIdentity::try_clone` (see its own doc comment in
    // `siar-crypto`) is exactly the "more than one owner of the same
    // key material in one process" case it exists for. Same pattern
    // `apps/cli`'s `bootstrap()` already uses.
    let group_identity = identity
        .try_clone()
        .context("cloning device identity for GroupService")?;
    // MLS group state lives on disk (one SQLite file per group, under
    // `<data dir>/mls`), so groups and the key package this device handed
    // out survive a restart. `restore_mls_sessions` reopens them; a group
    // that fails to reopen is logged and skipped, never fatal.
    let group_service = Arc::new(
        GroupService::new_persistent(
            device_id,
            local_account,
            group_identity,
            endpoint.clone(),
            messages.clone(),
            device_directory.clone(),
            groups,
            paths.data_dir.join("mls"),
        )
        .context("opening MLS group storage")?,
    );
    let restored_groups = group_service.restore_mls_sessions();

    // Publish once at startup, immediately reclaim the bytes from the
    // same in-process directory to display them — this device is the
    // only writer/reader of `key_package_directory` today (see that
    // field's own doc comment), so `take` right after `publish` always
    // succeeds unless publishing itself failed. A logged warning, not
    // a hard bootstrap failure, since losing "can be added to a group"
    // shouldn't take down 1:1 messaging too.
    let key_package_b64 = match group_service
        .ensure_key_package_published(key_package_directory.as_ref())
    {
        Ok(()) => key_package_directory.take(device_id).map(|bytes| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(bytes)
        }),
        Err(e) => {
            tracing::warn!(error = %e, "failed to publish this device's MLS key package at startup");
            None
        }
    };

    let service = Arc::new(MessageService::new(
        device_id, identity, endpoint, messages, outbox, blobs,
    ));
    Ok(Bootstrapped {
        service,
        group_service,
        my_ticket,
        incoming_rx: rx,
        local_account,
        device_id,
        key_package_directory,
        key_package_b64,
        device_directory,
        contact_repo,
        saved_contacts,
        messages: message_view,
        restored_groups,
        data_dir: paths.data_dir.clone(),
        identity_created_at_millis,
    })
}
