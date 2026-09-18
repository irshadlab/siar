//! The `siar_messaging::MessageService` FFI surface
//! `apps/android/README.md` named explicitly as missing: "there is no
//! `siar-messaging::MessageService` FFI surface here at all... This is
//! separate, much larger follow-up work." This crate is that follow-up
//! — deliberately still a *slice* of it, not the whole thing (see
//! "What's still NOT here" below).
//!
//! ## Shape: one global instance, a background pump, a poll queue
//!
//! Same "exactly one, not per-connection" reasoning
//! `siar-android-connectivity`'s own doc comment gives for
//! `ConnectivityState`: there's one `MessageService` for the whole
//! running app, so this is a `static` behind a lock, not a
//! `createBridge`/handle pattern like the four transport bridges use.
//!
//! JNI is call-in only in this whole app (every `jni_bridge.rs`'s own
//! doc comment states this rule) — Rust has no way to push an incoming
//! message to Kotlin the instant it arrives. So a background task
//! (spawned once, in [`bootstrap`]) drains `MessageService`'s incoming
//! frames continuously and appends a plain-text summary line to an
//! in-memory queue; [`poll_next_event`] is what Kotlin calls
//! (repeatedly, e.g. on its own timer — same tradeoff
//! [`com.siar.ble.BleGattManager`]'s fragment pump already documents
//! for the identical shape) to drain it one line at a time.
//!
//! ## Event line format
//!
//! Deliberately plain tab-separated text, not JSON — this workspace has
//! no `serde_json` dependency anywhere (checked: `siar-crypto-mls`'s own
//! Cargo.toml has a comment explaining why it deliberately avoided
//! adding one), and inventing a second serialization format across the
//! JNI boundary for a handful of fixed-shape lines is real, avoidable
//! overhead for both sides — Kotlin can `split("\t")` a fixed number of
//! fields without a JSON parser dependency of its own. Two event kinds
//! today:
//! - `text\t<sender endpoint id hex>\t<message text>`
//! - `mailbox\t<count>` — a `MailboxCheckIn` response batch finished
//!   arriving (matches `apps/cli`'s own `check_mailbox`'s "count and
//!   move on" shape; individual mailbox item contents aren't
//!   surfaced by this crate yet — see below).
//! - `anon_text\t<matched peer's endpoint id hex>\t<message text>` — a
//!   `TokenMailboxDeposit` that decrypted successfully against one of
//!   this process's registered peers. The "sender" here is *inferred*
//!   (which candidate's session key actually opened it), not read off
//!   the wire — an anonymous check-in response has no sender field at
//!   all, the entire point (see `siar_crypto::mailbox_token`'s doc
//!   comment).
//! - `attachment\t<sender endpoint id debug>\t<blob_hash b64>\t<size
//!   bytes>\t<media type>\t<attachment_key b64>` — a 1:1
//!   `MessageContent::Attachment` arrived; [`fetch_attachment`] needs
//!   every one of these fields back to actually retrieve the blob.
//! - `group_invite\t<conversation id>\t<from device debug>` — a
//!   `GroupMlsWelcome` arrived and is buffered, waiting on
//!   [`group_join`]/[`group_decline_invite`].
//! - `group_text\t<conversation id>\t<sender device debug>\t<message
//!   text>` — a `GroupMlsApplication` frame decoded to text.
//!
//! ## What's here
//!
//! - [`bootstrap`] — generates a fresh identity (no persistence across
//!   app restarts yet, same Phase-1 stand-in `apps/cli`'s own
//!   `bootstrap()` doc comment already carries), binds a
//!   `SiarEndpoint`, starts the incoming-event pump, and reports
//!   `TransportLink::InternetDirect` up into `siar-android-
//!   connectivity`'s shared `ConnectivityState` once the bind
//!   succeeds (no corresponding "mark it down" anywhere in this crate
//!   yet — see [`bootstrap`]'s own inline comment).
//! - [`my_ticket`] — this device's own `PeerTicket`, encoded, to
//!   display/share (e.g. as a QR code — not built here, just the
//!   string it would encode).
//! - [`add_peer`] — registers a peer's ticket so incoming `V1`
//!   envelopes from them can actually be decrypted (`MessageService::
//!   handle_incoming` needs the sender's `PeerTicket` up front — see
//!   that method's own doc comment: "Phase 1's CLI trusts that `peer`
//!   really is who the transport says it is... paired by hand," the
//!   exact same precondition this crate inherits unchanged). Also
//!   what lets an incoming `TokenMailboxDeposit` be matched to a
//!   sender — see the `anon_text` event kind above.
//! - [`send_text`]/[`send_text_anon`] — the `DeviceId`-addressed and
//!   unlinkable-token-addressed paths respectively
//!   (`MessageService::send_text`/`send_text_anon`), mirroring
//!   `apps/cli`'s `send`/`send-anon`.
//! - [`check_mailbox`]/[`check_mailbox_anon`] — fire a
//!   `MailboxCheckIn`/`AnonymousMailboxCheckIn` at a relay; responses
//!   land in the poll queue as `mailbox\t<count>`/`anon_text\t...`
//!   respectively, mirroring `apps/cli`'s `check-mailbox`/
//!   `check-mailbox-anon`.
//! - **Groups/MLS** — `GroupService` is now wired in, mirroring
//!   `apps/desktop`'s own `bootstrap_messaging`/`incoming_loop` wiring
//!   (the reference this crate's group functions were built against)
//!   rather than `apps/cli`'s one-shot-process version, since a
//!   long-running Android process can match the desktop app's
//!   always-listening shape instead of needing the CLI's manual
//!   `listen --publish-key-package` workaround for
//!   `pending_identity`'s in-process-only lifetime. A key package is
//!   published once automatically at [`bootstrap`] time (same as
//!   desktop) — [`group_key_package`] returns the resulting base64
//!   text to share out-of-band with whoever will add this device to a
//!   group, exactly the same hand-paired exchange `PeerTicket` itself
//!   already requires (see this crate's own top doc comment on that).
//!   [`group_create`]/[`group_add_member`]/[`group_send_text`]/
//!   [`group_join`]/[`group_decline_invite`] are the rest of the
//!   surface; an incoming `GroupMlsWelcome` is buffered (not
//!   auto-joined — same deliberate "let the UI ask first" stance
//!   `GroupService::handle_incoming_mls`'s own doc comment states) and
//!   surfaces as a `group_invite\t...` poll event, `GroupMlsApplication`
//!   text as `group_text\t...`. **Not attempted**: `add_member`/
//!   `remove_member` (the static-key, non-MLS group path —
//!   `create_group_mls`'s MLS path is the real cryptographic one, same
//!   choice `apps/cli`/`apps/desktop` both made), `remove_member_mls`,
//!   and group attachments (`handle_incoming_mls` can hand back
//!   `MessageContent::Attachment` in principle, but fetching it needs
//!   the sending device's `PeerTicket`, and `envelope.sender` is only
//!   a bare `DeviceId` with no directory lookup back to one exposed
//!   anywhere in this codebase yet, in `apps/desktop` either — a real
//!   gap in the library this crate wraps, not something to paper over
//!   here).
//! - **Attachments (1:1 only)** — [`send_attachment`]/
//!   [`fetch_attachment`] wrap `MessageService::send_attachment`/
//!   `fetch_attachment` directly (both already take/return plain
//!   `Vec<u8>`, so unlike groups there's no out-of-band exchange
//!   needed here — the reference and the blob both travel over the
//!   wire on their own). An incoming `MessageContent::Attachment`
//!   surfaces as `attachment\t...` in the poll queue, carrying the
//!   `AttachmentReference` fields [`fetch_attachment`] needs back.
//!   Deliberately no image decoding/thumbnailing here (unlike
//!   `apps/desktop`'s `siar-media-image`-backed preview pipeline) —
//!   Android already prefers platform codecs over this workspace's own
//!   native ones (see `build-native.sh`'s own comment on why
//!   `siar-media-audio`/`siar-media-av1` are desktop-only), and
//!   `BitmapFactory` on the Kotlin side is the equivalent tool for
//!   images specifically, not a new native dependency here.
//!
//! ## What's still NOT here
//!
//! - **No delivery-ack visibility.** Incoming `DeliveryAck`/
//!   `ReadReceipt` frames are silently absorbed by `handle_incoming`
//!   (returns `None` for them, same as `apps/cli`) — nothing surfaces
//!   "your message was delivered" to Kotlin.
//! - **Individual mailbox item contents aren't surfaced.** `check_mailbox`
//!   reports a count, not each `Mesh` envelope's ciphertext/metadata —
//!   `apps/cli`'s `check_mailbox` shows byte length/hop_limit/priority
//!   per item; this crate's poll-queue line format doesn't have room
//!   for that yet without designing a richer event line (or, more
//!   likely, finally justifying a real serialization format — noted,
//!   not decided here).
//! - **Identity/database now persist across restarts.** `bootstrap`
//!   takes a `base_dir` (Kotlin passes `Context.filesDir`) and
//!   load-or-creates `DeviceIdentity`/`DeviceId`/`AccountId` there via
//!   `DeviceIdentity::save_to_file`/`load_from_file` (already-existing
//!   primitives, previously with no Android caller) plus two bare-UUID
//!   text files, and opens a real on-disk database (`siar_storage::open`)
//!   instead of `open_in_memory()` — the exact pattern `apps/desktop`'s
//!   own `resolve_data_paths`/`load_or_create_id` already used,
//!   applied here for the first time, and later mirrored into
//!   `apps/cli` too (see that binary's own `resolve_data_paths`/
//!   `load_or_create_id`) — all three of this workspace's client
//!   entry points now persist identity the same way.
//! - **`mark_link_up` now has a real `mark_link_down` counterpart** —
//!   [`shutdown_inner`], called (via JNI) from `MainActivity.onDestroy`.
//!   `APP` moved from a `OnceLock` to `Mutex<Option<Arc<AppMessaging>>>`
//!   specifically so this could stop the pump task
//!   (`JoinHandle::abort`) and let the bound endpoint drop for real,
//!   instead of only updating connectivity state — see
//!   [`shutdown_inner`]'s own doc comment for the one remaining
//!   caveat (abort isn't synchronous).

// Everything below `AppMessaging` through `poll_next_event_inner` is
// only ever called from `jni_bridge`, which is `#[cfg(target_os =
// "android")]` — on a host `cargo check`/`cargo test` (this crate's
// `rlib` output, not its `cdylib`), that whole module drops out and
// every one of these becomes unreachable dead code by the compiler's
// reckoning, despite being real, exercised logic on the platform this
// crate actually ships on. Same reasoning, same fix shape,
// `siar-android-connectivity::link_from_ordinal`'s own `cfg_attr`
// already uses — applied once here at module scope instead of item by
// item, since nearly everything in this file shares the exact same
// android-only-caller situation.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use siar_domain::{AccountId, ConversationId, DeviceId, MediaType, MessageContent, MessageText};
use siar_messaging::{
    GroupService, InMemoryDeviceDirectory, InMemoryKeyPackageDirectory, IncomingEvent,
    KeyPackageDirectory, MemberDevice, MessageService, PeerTicket,
};
use siar_protocol::v1::EnvelopeKind;
use siar_transport::{IncomingFrame, PeerTransport, SiarEndpoint};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

/// Real evidence-based link classification — moved here from the
/// retired `siar_routing::path::classify_endpoint_addr` (see
/// `MIGRATION.md`'s device-certificate/reconciliation notes for why
/// `siar-routing` no longer exists). Same private/public-IP heuristic,
/// same documented limitation (an advertised LAN IP actually reached
/// over relay still classifies as `LocalLan`), just returning
/// `siar_domain::TransportLink` directly rather than
/// `siar_routing_policy::TransportKind` — this crate reports link
/// status straight into `siar_android_connectivity::ConnectivityState`
/// (a `TransportLink`-typed API), so there's no reason to pull in
/// `siar-connectivity`/`siar-routing-policy`'s heavier dependency
/// chain (transitively, `siar-protocol-ext` and friends) into an
/// Android `cdylib` just to re-derive `TransportKind` and then map it
/// straight back down to `TransportLink` again.
fn classify_endpoint_addr(addr: &iroh::EndpointAddr) -> siar_domain::TransportLink {
    let mut saw_any_ip = false;
    for socket_addr in addr.ip_addrs() {
        saw_any_ip = true;
        if is_local_or_private_ip(socket_addr) {
            return siar_domain::TransportLink::LocalLan;
        }
    }
    if saw_any_ip {
        siar_domain::TransportLink::InternetDirect
    } else {
        siar_domain::TransportLink::InternetRelay
    }
}

fn is_local_or_private_ip(socket_addr: &std::net::SocketAddr) -> bool {
    match socket_addr.ip() {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())
}

fn parse_uuid(s: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(s.trim()).map_err(|e| format!("'{s}' is not a valid id: {e}"))
}

fn media_type_from_str(s: &str) -> MediaType {
    // A conservative parse of `MediaType`'s own allow-list (see that
    // enum's doc comment) — an unrecognized string from Kotlin becomes
    // `Other` rather than a hard error, matching `MediaType::Other`'s
    // own stated purpose ("covers anything else without letting an
    // arbitrary string masquerade as a trusted type").
    match s {
        "image/png" => MediaType::ImagePng,
        "image/jpeg" => MediaType::ImageJpeg,
        "image/webp" => MediaType::ImageWebp,
        "audio/opus" => MediaType::AudioOpus,
        "video/mp4" => MediaType::VideoMp4,
        _ => MediaType::Other,
    }
}

fn media_type_to_str(m: MediaType) -> &'static str {
    match m {
        MediaType::ImagePng => "image/png",
        MediaType::ImageJpeg => "image/jpeg",
        MediaType::ImageWebp => "image/webp",
        MediaType::AudioOpus => "audio/opus",
        MediaType::VideoMp4 => "video/mp4",
        MediaType::Other => "application/octet-stream",
    }
}

struct AppMessaging {
    endpoint: Arc<SiarEndpoint>,
    service: MessageService,
    my_ticket: PeerTicket,
    device_id: DeviceId,
    local_account: AccountId,
    /// Registered via [`add_peer`] — see this module's top doc comment
    /// on why a sender's `PeerTicket` has to already be known before an
    /// incoming `V1` envelope from them can be decrypted.
    known_peers: Mutex<HashMap<iroh::EndpointId, PeerTicket>>,
    events: Mutex<VecDeque<String>>,
    /// The incoming-frame pump's own handle, so [`shutdown_inner`] can
    /// actually stop it via [`tokio::task::JoinHandle::abort`] instead
    /// of leaving it running until process exit — the specific
    /// limitation this field exists to close. `Mutex<Option<_>>`
    /// rather than a bare field since it's populated right after
    /// `tokio::spawn` returns, one step after this struct itself is
    /// constructed (see [`bootstrap_inner`]).
    pump_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    group_service: GroupService,
    /// Fanout targets for `add_member_mls`/`remove_member_mls` — see
    /// `GroupService::DeviceDirectory`'s own doc comment. Populated by
    /// [`group_add_member_inner`] right before calling `add_member_mls`,
    /// same order `apps/cli`'s `group_add_member` uses.
    device_directory: Arc<InMemoryDeviceDirectory>,
    /// Backs [`group_publish_key_package_inner`] (called once at
    /// [`bootstrap_inner`] time, same as `apps/desktop`'s own
    /// `bootstrap_messaging`) — see `GroupService::publish_key_package`'s
    /// doc comment for why this is only ever a same-process, in-memory
    /// directory here (no cross-device key-package discovery exists in
    /// this codebase yet).
    key_package_directory: Arc<InMemoryKeyPackageDirectory>,
    /// This device's own published key package, base64-encoded, ready
    /// to share out-of-band with whoever will call [`group_add_member`]
    /// for this device — computed once at bootstrap (mirrors desktop's
    /// `key_package_b64`), empty if publishing failed at bootstrap time
    /// (a logged warning there, not a hard bootstrap failure — same
    /// "losing 'can be added to a group' shouldn't take down 1:1
    /// messaging too" reasoning desktop's own comment gives).
    key_package_b64: String,
    /// A `GroupMlsWelcome`'s payload, buffered by conversation id until
    /// [`group_join_inner`] (or [`group_decline_invite_inner`]) consumes
    /// it — mirrors `apps/desktop`'s `PendingInviteState`/`PendingInvite`
    /// (`siar-ui-state`), reimplemented here rather than pulled in as a
    /// dependency since this crate's own event-line/poll-queue shape
    /// already replaces what that crate's `Signal`-friendly structs are
    /// for. `(DeviceId, Vec<u8>)`: the welcome's sender (`envelope.sender`,
    /// for display) alongside the welcome bytes themselves.
    pending_welcomes: Mutex<HashMap<ConversationId, (DeviceId, Vec<u8>)>>,
}

// `RUNTIME` stays a `OnceLock`: a Tokio multi-thread runtime is a real
// thread pool, and tearing it down and rebuilding it on every
// bootstrap/shutdown cycle is neither how any real Android app treats
// its executor nor something a `shutdown()` call driven from
// `onDestroy` needs — the process is going away regardless once that
// fires for real. `APP` is the piece that actually needed to become
// resettable: it holds the per-session `SiarEndpoint`/`MessageService`/
// pump task, which a real `shutdown()` must be able to tear down and
// let a later `bootstrap()` recreate from scratch, which a `OnceLock`
// can never do once set. `Arc` (not a bare `AppMessaging`) so the pump
// task can hold its own strong reference independent of the slot in
// `APP`, which [`shutdown_inner`] can empty out from under it.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static APP: Mutex<Option<Arc<AppMessaging>>> = Mutex::new(None);

/// Clones the current app handle out of the lock and releases it
/// immediately — every caller below needs the `Arc` for the duration
/// of a `block_on`/lock elsewhere, not the `APP` mutex itself, so
/// nothing holds `APP`'s lock across an `.await` or another lock
/// acquisition.
fn app_handle() -> Option<Arc<AppMessaging>> {
    APP.lock().expect("APP lock poisoned").clone()
}

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build the Tokio runtime this crate drives every async call through")
    })
}

/// Where this device's identity/account id/database live on disk —
/// same fix `apps/desktop`'s own `DataPaths`/`resolve_data_paths`
/// already carries, applied here for the first time on Android: before
/// this, `bootstrap_inner` regenerated a fresh `DeviceIdentity`/
/// `AccountId`/`DeviceId` and opened an in-memory database on *every*
/// launch, so nothing survived a process restart — the specific gap
/// this crate's own doc comment and `apps/android/README.md` both
/// named. `base_dir` is supplied by Kotlin (`Context.filesDir`,
/// app-private storage the OS already sandboxes per-app — no new
/// permission needed), since this crate has no `Context` of its own to
/// resolve a directory from.
struct AppDataPaths {
    identity: std::path::PathBuf,
    account_id: std::path::PathBuf,
    device_id: std::path::PathBuf,
    database: std::path::PathBuf,
}

impl AppDataPaths {
    fn under(base_dir: &str) -> Self {
        let base = std::path::PathBuf::from(base_dir);
        Self {
            identity: base.join("identity.bin"),
            account_id: base.join("account_id.txt"),
            device_id: base.join("device_id.txt"),
            database: base.join("siar.db"),
        }
    }
}

/// Same "bare UUID text file, not postcard/JSON" reasoning
/// `apps/desktop`'s own `load_or_create_id` doc comment gives —
/// duplicated here rather than shared, since sharing it would mean
/// either `apps/desktop` depending on this crate or a new shared crate
/// for one ~10-line function, neither of which is worth the coupling.
fn load_or_create_id<T>(
    path: &std::path::Path,
    from_uuid: impl Fn(uuid::Uuid) -> T,
    to_uuid: impl Fn(&T) -> uuid::Uuid,
    generate: impl Fn() -> T,
) -> Result<T, String> {
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let id = uuid::Uuid::parse_str(text.trim()).map_err(|e| e.to_string())?;
        Ok(from_uuid(id))
    } else {
        let id = generate();
        std::fs::write(path, to_uuid(&id).to_string()).map_err(|e| e.to_string())?;
        Ok(id)
    }
}

/// Real setup, real errors — every fallible step returns a `String`
/// description rather than panicking, since a JNI function panicking
/// unwinds into Kotlin as undefined behavior (documented Rust/JNI
/// interop hazard, not specific to this crate) rather than a catchable
/// exception.
fn bootstrap_inner(base_dir: &str) -> Result<String, String> {
    if let Some(app) = app_handle() {
        return Ok(app.my_ticket.encode());
    }
    let app_arc: Arc<AppMessaging> = runtime().block_on(async {
        std::fs::create_dir_all(base_dir).map_err(|e| e.to_string())?;
        let paths = AppDataPaths::under(base_dir);

        let identity = if paths.identity.exists() {
            siar_crypto::DeviceIdentity::load_from_file(&paths.identity).map_err(|e| e.to_string())?
        } else {
            let identity = siar_crypto::DeviceIdentity::generate();
            identity.save_to_file(&paths.identity).map_err(|e| e.to_string())?;
            identity
        };
        let device_id = load_or_create_id(&paths.device_id, DeviceId::from_uuid, DeviceId::as_uuid, DeviceId::new)?;
        let local_account = load_or_create_id(&paths.account_id, AccountId::from_uuid, AccountId::as_uuid, AccountId::new)?;

        // Real on-disk database, not `open_in_memory()` — the other
        // half of the persistence fix: an in-memory DB would have
        // discarded every message/outbox row the moment the process
        // exited regardless of how stable the identity above now is.
        let db = siar_storage::open(&paths.database.display().to_string()).map_err(|e| e.to_string())?;
        let messages = Arc::new(siar_storage::StoolapMessageRepository::new(db.clone()));
        let outbox = Arc::new(siar_storage::StoolapOutboxRepository::new(db.clone()));
        let blobs: Arc<dyn siar_storage::BlobRepository + Send + Sync> =
            Arc::new(siar_storage::StoolapBlobRepository::new(db.clone()));
        let blob_store: Arc<dyn siar_transport::BlobStore> =
            Arc::new(siar_messaging::StorageBlobStore(blobs.clone()));
        let groups: Arc<dyn siar_storage::GroupRepository + Send + Sync> =
            Arc::new(siar_storage::StoolapGroupRepository::new(db));

        let (tx, mut rx) = mpsc::channel::<IncomingFrame>(64);
        let iroh_secret = iroh::SecretKey::generate();
        let endpoint = Arc::new(SiarEndpoint::bind(iroh_secret, tx, blob_store).await.map_err(|e| e.to_string())?);

        let my_ticket = PeerTicket {
            endpoint_addr: endpoint.addr(),
            x25519_public: identity.x25519_public().to_bytes(),
            ed25519_verifying: identity.verifying_key().to_bytes(),
        };

        // Closes the gap this crate's own doc comment (and
        // `apps/android/README.md`) used to name explicitly: a
        // successfully bound `SiarEndpoint` is real evidence this
        // device has Internet/LAN connectivity via iroh, reported into
        // the same shared `ConnectivityState` the four transport
        // bridges already feed. [`shutdown_inner`] now has a real
        // `mark_link_down` counterpart to this.
        siar_android_connectivity::mark_link_up(siar_domain::TransportLink::InternetDirect);

        // `MessageService` and `GroupService` both take a
        // `DeviceIdentity` by value, and both represent this same local
        // device — `DeviceIdentity::try_clone` is exactly the "more
        // than one owner of the same key material in one process" case
        // it exists for. Same pattern `apps/cli`'s `bootstrap()` and
        // `apps/desktop`'s `bootstrap_messaging` both already use.
        let group_identity = identity.try_clone().map_err(|e| e.to_string())?;
        let device_directory = Arc::new(InMemoryDeviceDirectory::new());
        let key_package_directory = Arc::new(InMemoryKeyPackageDirectory::new());
        let group_service = GroupService::new(
            device_id,
            local_account,
            group_identity,
            endpoint.clone(),
            messages.clone(),
            device_directory.clone(),
            groups,
        );

        // Publish once at startup, immediately reclaim the bytes from
        // the same in-process directory to hand back to Kotlin — this
        // device is the only writer/reader of `key_package_directory`
        // today (see that field's own doc comment), so `take` right
        // after `publish` always succeeds unless publishing itself
        // failed. A logged warning, not a hard bootstrap failure, since
        // losing "can be added to a group" shouldn't take down 1:1
        // messaging too — same reasoning `apps/desktop`'s own comment
        // gives for this exact sequence.
        let key_package_b64 = match group_service.publish_key_package(key_package_directory.as_ref()) {
            Ok(()) => key_package_directory.take(device_id).map(|bytes| base64_encode(&bytes)).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to publish this device's MLS key package at startup");
                String::new()
            }
        };

        let service = MessageService::new(device_id, identity, endpoint.clone(), messages, outbox, blobs);

        let app = AppMessaging {
            endpoint,
            service,
            my_ticket: my_ticket.clone(),
            device_id,
            local_account,
            group_service,
            device_directory,
            key_package_directory,
            key_package_b64,
            pending_welcomes: Mutex::new(HashMap::new()),
            known_peers: Mutex::new(HashMap::new()),
            events: Mutex::new(VecDeque::new()),
            pump_handle: Mutex::new(None),
        };
        let app = Arc::new(app);

        // The incoming-frame pump — see this module's top doc comment
        // for why this drains continuously into a poll queue instead
        // of pushing into Kotlin directly. Captures its own `Arc`
        // clone of `app` directly rather than looking it up via a
        // global on every frame — the pump no longer depends on `APP`
        // being populated at all, which is what makes it possible for
        // [`shutdown_inner`] to empty `APP` out from under it and stop
        // it independently via the handle stored just below.
        let pump_app = app.clone();
        let pump_handle = tokio::spawn(async move {
            let app = pump_app;
            let mut mailbox_batch_count: u32 = 0;
            loop {
                let Some(frame) = rx.recv().await else { break };
                match frame.message {
                    siar_protocol::WireMessage::V1(envelope) => {
                        // Group frames go to `GroupService`, not
                        // `MessageService::handle_incoming` — that
                        // function explicitly doesn't handle these (see
                        // its own doc comment) — checked before the
                        // 1:1 `known_peers` lookup below since group
                        // frames don't need (or use) a registered 1:1
                        // `PeerTicket` at all. Mirrors `apps/desktop`'s
                        // `incoming_loop` gate exactly (see this
                        // module's top doc comment on why that app, not
                        // `apps/cli`, is what this crate's group support
                        // was built against).
                        if matches!(
                            envelope.kind,
                            EnvelopeKind::GroupEvent
                                | EnvelopeKind::GroupMlsCommit
                                | EnvelopeKind::GroupMlsWelcome
                                | EnvelopeKind::GroupMlsApplication
                        ) {
                            match envelope.kind {
                                EnvelopeKind::GroupMlsWelcome => {
                                    // Buffered, not auto-joined — see
                                    // `GroupService::handle_incoming_mls`'s
                                    // own doc comment on why a welcome
                                    // never joins itself. `group_join`
                                    // is what a person explicitly
                                    // choosing to accept calls, taking
                                    // this back out.
                                    app.pending_welcomes
                                        .lock()
                                        .expect("pending_welcomes lock poisoned")
                                        .insert(envelope.conversation_id, (envelope.sender, envelope.payload));
                                    let line = format!("group_invite\t{}\t{:?}", envelope.conversation_id, envelope.sender);
                                    app.events.lock().expect("events lock poisoned").push_back(line);
                                }
                                _ => match app.group_service.handle_incoming_mls(envelope.conversation_id, &envelope) {
                                    Ok(Some(MessageContent::Text(text))) => {
                                        let line = format!(
                                            "group_text\t{}\t{:?}\t{}",
                                            envelope.conversation_id,
                                            envelope.sender,
                                            text.as_str()
                                        );
                                        app.events.lock().expect("events lock poisoned").push_back(line);
                                    }
                                    Ok(Some(MessageContent::Attachment(_))) => {
                                        // A real, named gap, not a
                                        // silent drop — see this crate's
                                        // top doc comment's "Not
                                        // attempted: ... group
                                        // attachments" bullet for why
                                        // this can't be surfaced the
                                        // same way 1:1 attachments are
                                        // below: `envelope.sender` is a
                                        // bare `DeviceId` with no
                                        // directory lookup back to a
                                        // fetchable `PeerTicket`
                                        // anywhere in this codebase yet.
                                        tracing::warn!(
                                            conversation = ?envelope.conversation_id,
                                            "a group attachment arrived — dropping, no DeviceId-to-PeerTicket lookup exists yet to fetch it with"
                                        );
                                    }
                                    Ok(_) => {} // commit merged, call-signal, or nothing to show
                                    Err(e) => tracing::warn!(error = %e, conversation = ?envelope.conversation_id, "failed to handle incoming group frame"),
                                },
                            }
                            continue;
                        }

                        let peer_ticket = {
                            let known = app.known_peers.lock().expect("known_peers lock poisoned");
                            known.get(&frame.from).cloned()
                        };
                        let Some(peer_ticket) = peer_ticket else {
                            tracing::debug!(from = ?frame.from, "V1 envelope from an unregistered peer, dropping — call add_peer first");
                            continue;
                        };
                        match app.service.handle_incoming(&peer_ticket, envelope).await {
                            Ok(Some(IncomingEvent::Content(MessageContent::Text(text)))) => {
                                // `{:?}` (Debug), not `{}` (Display) —
                                // matching this workspace's own existing
                                // precedent for formatting an
                                // `iroh::EndpointId` (every `tracing::
                                // debug!(from = ?frame.from, ...)` in
                                // `apps/emergency-node`, for instance):
                                // whether `PublicKey` implements
                                // `Display` wasn't confirmed against
                                // real docs this pass, so this doesn't
                                // assume it does.
                                let line = format!("text\t{:?}\t{}", frame.from, text.as_str());
                                app.events.lock().expect("events lock poisoned").push_back(line);
                            }
                            Ok(Some(IncomingEvent::Content(MessageContent::Attachment(reference)))) => {
                                // Carries back exactly what
                                // `fetch_attachment_inner` needs to
                                // actually retrieve the blob — see
                                // `AttachmentReference`'s own field doc
                                // comments for what each of these is.
                                let line = format!(
                                    "attachment\t{:?}\t{}\t{}\t{}\t{}",
                                    frame.from,
                                    base64_encode(&reference.blob_hash),
                                    reference.encrypted_size.bytes(),
                                    media_type_to_str(reference.media_type),
                                    base64_encode(&reference.attachment_key),
                                );
                                app.events.lock().expect("events lock poisoned").push_back(line);
                            }
                            Ok(_) => {} // call-signal/ack — not surfaced by this crate yet, see top doc comment
                            Err(e) => tracing::debug!(error = %e, from = ?frame.from, "failed to handle an incoming V1 envelope"),
                        }
                    }
                    siar_protocol::WireMessage::Mesh(_) => {
                        mailbox_batch_count += 1;
                    }
                    siar_protocol::WireMessage::TokenMailboxDeposit(envelope) => {
                        // An answer to `check_mailbox_anon`. The wire
                        // message itself carries no sender identity —
                        // that's the entire unlinkability point (see
                        // `siar_crypto::mailbox_token`'s doc comment) —
                        // so the only way to find out who it's from is
                        // to try decrypting against every peer this
                        // process currently knows about (the same
                        // `known_peers` registered via `add_peer`) and
                        // see which one's session actually opens it.
                        // AEAD decryption rejecting a wrong key is a
                        // real, correct signal here, not a workaround —
                        // the same principle every "try each candidate
                        // key" pattern in real E2EE clients relies on.
                        let candidates: Vec<PeerTicket> = {
                            let known = app.known_peers.lock().expect("known_peers lock poisoned");
                            known.values().cloned().collect()
                        };
                        let mut matched = false;
                        for candidate in candidates {
                            if let Ok(MessageContent::Text(text)) =
                                app.service.decrypt_token_mailbox_envelope(&candidate, &envelope)
                            {
                                let line = format!("anon_text\t{:?}\t{}", candidate.endpoint_addr.id, text.as_str());
                                app.events.lock().expect("events lock poisoned").push_back(line);
                                matched = true;
                                break;
                            }
                        }
                        if !matched {
                            tracing::debug!("received a TokenMailboxDeposit that didn't decrypt against any known peer");
                        }
                    }
                    _ => {} // MailboxCheckIn/AnonymousMailboxCheckIn/RouteAdvertisement — not this client's traffic (it only ever sends the first two, never receives them back; RouteAdvertisement is relay-to-relay)
                }
                // A crude batch boundary: report whatever mailbox items
                // have accumulated once the channel briefly empties,
                // rather than trying to detect "the relay is done
                // sending" any more precisely — `try_recv` draining
                // what's immediately available is a real, if approximate,
                // stand-in for `apps/cli::check_mailbox`'s fixed 5-second
                // deadline (this crate has no equivalent timer here).
                if mailbox_batch_count > 0 {
                    while rx.try_recv().is_ok() {
                        mailbox_batch_count += 1;
                    }
                    let line = format!("mailbox\t{mailbox_batch_count}");
                    app.events.lock().expect("events lock poisoned").push_back(line);
                    mailbox_batch_count = 0;
                }
            }
        });
        *app.pump_handle.lock().expect("pump_handle lock poisoned") = Some(pump_handle);

        Ok::<Arc<AppMessaging>, String>(app)
    })?;

    // Installed only now that everything above succeeded — `APP` never
    // sees a partially-built `AppMessaging`. If another call already
    // won the race and installed first, this call's own endpoint/pump
    // are real but redundant: stop its pump immediately and let its
    // `Arc` drop (see `shutdown_inner`'s own comment on why `abort`
    // doesn't guarantee the drop lands synchronously — same caveat
    // here) rather than erroring the caller out, since the winner's
    // ticket is just as valid an answer to "what's my ticket".
    let mut guard = APP.lock().expect("APP lock poisoned");
    if let Some(existing) = guard.as_ref() {
        let ticket = existing.my_ticket.encode();
        drop(guard);
        if let Some(handle) = app_arc
            .pump_handle
            .lock()
            .expect("pump_handle lock poisoned")
            .take()
        {
            handle.abort();
        }
        return Ok(ticket);
    }
    let ticket = app_arc.my_ticket.encode();
    *guard = Some(app_arc);
    Ok(ticket)
}

fn my_ticket_inner() -> String {
    app_handle()
        .expect("bootstrap must run before my_ticket")
        .my_ticket
        .encode()
}

fn add_peer_inner(ticket: &str) -> Result<(), String> {
    let app = app_handle().ok_or("bootstrap must run before add_peer")?;
    let peer = PeerTicket::decode(ticket).map_err(|e| e.to_string())?;
    app.known_peers
        .lock()
        .expect("known_peers lock poisoned")
        .insert(peer.endpoint_addr.id, peer);
    Ok(())
}

fn send_text_inner(peer_ticket: &str, text: &str) -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before send_text")?;
    let peer = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let text = MessageText::parse(text.to_string()).map_err(|e| e.to_string())?;
    // Real evidence-based link classification, replacing the bootstrap-
    // time blanket `InternetDirect` — see this file's own
    // `classify_endpoint_addr`'s doc comment for exactly what this is
    // (advertised reachability) and isn't (a measured path).
    siar_android_connectivity::mark_link_up(classify_endpoint_addr(&peer.endpoint_addr));
    runtime().block_on(async {
        // Phase-1 stand-in, same as `apps/cli`'s own `send`: a real
        // client looks up (or creates) the conversation with this peer
        // rather than minting a fresh one per call.
        let conversation = ConversationId::new();
        app.service
            .send_text(conversation, &peer, text)
            .await
            .map(|id| id.to_string())
            .map_err(|e| e.to_string())
    })
}

fn check_mailbox_inner(relay_ticket: &str) -> Result<(), String> {
    let app = app_handle().ok_or("bootstrap must run before check_mailbox")?;
    let relay = PeerTicket::decode(relay_ticket).map_err(|e| e.to_string())?;
    let check_in = app.service.sign_mailbox_check_in(siar_domain::now_millis());
    runtime().block_on(async {
        app.endpoint
            .send(
                relay.endpoint_addr.clone(),
                &siar_protocol::WireMessage::MailboxCheckIn(check_in),
            )
            .await
            .map_err(|e| e.to_string())
    })
}

/// The unlinkable counterpart to [`send_text_inner`] — see
/// `MessageService::send_text_anon`'s own doc comment for exactly what
/// this does and doesn't guarantee (no delivery ack/retry on this
/// path, in particular). `peer_ticket` is who the message is *for*;
/// `relay_ticket` is who it's handed to for pickup.
fn send_text_anon_inner(
    peer_ticket: &str,
    relay_ticket: &str,
    text: &str,
) -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before send_text_anon")?;
    let peer = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let relay = PeerTicket::decode(relay_ticket).map_err(|e| e.to_string())?;
    let text = MessageText::parse(text.to_string()).map_err(|e| e.to_string())?;
    // Classified against the relay's address, not the peer's — this
    // path is delivered *through* the relay (see `send_text_anon`'s
    // own doc comment), so the relay is the actual link this device
    // talks over, same reasoning `classify_endpoint_addr` applies
    // anywhere else in this workspace: classify what's actually
    // reached, not the final recipient the traffic is addressed to.
    siar_android_connectivity::mark_link_up(classify_endpoint_addr(&relay.endpoint_addr));
    runtime().block_on(async {
        app.service
            .send_text_anon(&peer, &relay, text)
            .await
            .map(|id| id.to_string())
            .map_err(|e| e.to_string())
    })
}

/// The unlinkable counterpart to [`check_mailbox_inner`] — presents a
/// rotating token derived against `peer_ticket` instead of this
/// device's own `DeviceId`. Responses land in the poll queue as
/// `anon_text\t...` lines (see the incoming-frame pump's own comment
/// on `WireMessage::TokenMailboxDeposit` for how the sender is
/// recovered without a sender field on the wire). `peer_ticket` must
/// already be registered via [`add_peer_inner`] — the pump can't try
/// decrypting against a peer it doesn't know about.
fn check_mailbox_anon_inner(peer_ticket: &str, relay_ticket: &str) -> Result<(), String> {
    let app = app_handle().ok_or("bootstrap must run before check_mailbox_anon")?;
    let peer = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let relay = PeerTicket::decode(relay_ticket).map_err(|e| e.to_string())?;
    let check_in = app
        .service
        .build_anonymous_check_in(&peer, siar_domain::now_millis());
    runtime().block_on(async {
        app.endpoint
            .send(
                relay.endpoint_addr.clone(),
                &siar_protocol::WireMessage::AnonymousMailboxCheckIn(check_in),
            )
            .await
            .map_err(|e| e.to_string())
    })
}

/// Real, previously-missing piece the chat UI needs: Kotlin has no way
/// to decode a `PeerTicket` string itself (that's real Rust-side
/// decoding logic, not duplicated here), so without this there was no
/// way for a contact added via [`add_peer_inner`] to be matched back
/// against [`poll_next_event_inner`]'s `text\t<sender endpoint id
/// hex>\t...` lines — those two strings (a `PeerTicket` and a
/// `{:?}`-formatted `iroh::EndpointId`) look nothing alike even though
/// they can identify the same peer. This decodes a ticket and returns
/// the exact same `{:?}` (Debug) formatting the incoming-frame pump
/// already uses for `frame.from`, so a Kotlin-side contact list can
/// key its threads by this value and actually match incoming messages
/// to the contact that sent them. Pure — doesn't require [`bootstrap`]
/// to have run first, since it only decodes the ticket string itself.
fn ticket_endpoint_debug_inner(ticket: &str) -> Result<String, String> {
    let peer = PeerTicket::decode(ticket).map_err(|e| e.to_string())?;
    Ok(format!("{:?}", peer.endpoint_addr.id))
}

/// This device's own `DeviceId`/`AccountId`, as plain UUID text — what
/// [`group_add_member`]'s caller (the group admin, a different device)
/// needs alongside [`group_key_package`] to actually add this device to
/// a group, mirroring `apps/desktop`'s own "device id / account id /
/// ticket / key package" bundle (`publish_key_package`'s CLI
/// counterpart prints exactly these four things).
fn device_id_inner() -> Result<String, String> {
    Ok(app_handle()
        .ok_or("bootstrap must run before device_id")?
        .device_id
        .as_uuid()
        .to_string())
}

fn account_id_inner() -> Result<String, String> {
    Ok(app_handle()
        .ok_or("bootstrap must run before account_id")?
        .local_account
        .as_uuid()
        .to_string())
}

/// This device's own base64-encoded MLS key package, published once at
/// [`bootstrap`] — see [`AppMessaging::key_package_b64`]'s own doc
/// comment. Empty string (not an error) if publishing failed at
/// bootstrap time, matching that field's own "logged warning, not a
/// hard failure" choice.
fn group_key_package_inner() -> Result<String, String> {
    Ok(app_handle()
        .ok_or("bootstrap must run before group_key_package")?
        .key_package_b64
        .clone())
}

/// Creates a new MLS group with this device's account as founder/admin
/// — `GroupService::create_group_mls`. Returns the new conversation id
/// (its `Display` impl, same as `apps/cli`'s `group_create` prints) to
/// share with whoever will be added via [`group_add_member`].
fn group_create_inner() -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before group_create")?;
    let conversation = ConversationId::new();
    app.group_service
        .create_group_mls(conversation, app.local_account)
        .map_err(|e| e.to_string())?;
    Ok(conversation.to_string())
}

/// Admin-only (enforced inside `add_member_mls` itself, not re-checked
/// here). Registers the new member's device in [`AppMessaging::device_directory`]
/// (needed for fanout — see that field's own doc comment) and calls
/// `GroupService::add_member_mls`, which sends the MLS commit to every
/// existing member and the welcome to the new member over the wire.
/// Returns the base64-encoded post-admission `GroupState` — the piece
/// that does *not* travel over the wire (see `join_group_mls`'s own doc
/// comment for why) — for the caller to relay to the new member
/// out-of-band alongside the conversation id, exactly the shape
/// `apps/cli`'s `group_add_member` prints for pasting into `join-group`.
fn group_add_member_inner(
    conversation: &str,
    peer_ticket: &str,
    peer_device_id: &str,
    peer_account_id: &str,
    key_package_b64: &str,
) -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before group_add_member")?;
    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let peer_device = DeviceId::from_uuid(parse_uuid(peer_device_id)?);
    let peer_account = AccountId::from_uuid(parse_uuid(peer_account_id)?);
    let ticket = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let key_package_bytes = base64_decode(key_package_b64)?;

    app.device_directory.register(
        peer_account,
        MemberDevice {
            device_id: peer_device,
            ticket,
        },
    );

    runtime()
        .block_on(app.group_service.add_member_mls(
            conversation,
            peer_account,
            peer_device,
            &key_package_bytes,
        ))
        .map_err(|e| e.to_string())?;

    let state = app
        .group_service
        .group_state(conversation)
        .map_err(|e| e.to_string())?
        .ok_or("just-updated group has no local state — this is a bug")?;
    let state_bytes = postcard::to_allocvec(&state).map_err(|e| e.to_string())?;
    Ok(base64_encode(&state_bytes))
}

fn group_send_text_inner(conversation: &str, text: &str) -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before group_send_text")?;
    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let text = MessageText::parse(text.to_string()).map_err(|e| e.to_string())?;
    let message_id = runtime()
        .block_on(app.group_service.send_text_mls(conversation, text))
        .map_err(|e| e.to_string())?;
    Ok(message_id.to_string())
}

/// Consumes a buffered [`AppMessaging::pending_welcomes`] entry (the
/// `GroupMlsWelcome` bytes the pump already received over the wire —
/// see that field's own doc comment) together with a base64
/// `GroupState` obtained out-of-band from whoever called
/// [`group_add_member`] (their return value), and actually joins —
/// `GroupService::join_group_mls`. Errors with a clear message, not a
/// panic, if there's no pending invite for this conversation (already
/// joined, already declined, or the welcome hasn't arrived yet).
fn group_join_inner(conversation: &str, group_state_b64: &str) -> Result<(), String> {
    let app = app_handle().ok_or("bootstrap must run before group_join")?;
    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let (_from_device, welcome_bytes) = app
        .pending_welcomes
        .lock()
        .expect("pending_welcomes lock poisoned")
        .remove(&conversation)
        .ok_or("no pending invite for this conversation — wait for a group_invite event, or ask the admin to add you again")?;
    let state_bytes = base64_decode(group_state_b64)?;
    let state: siar_domain::GroupState =
        postcard::from_bytes(&state_bytes).map_err(|e| e.to_string())?;
    app.group_service
        .join_group_mls(conversation, &welcome_bytes, state)
        .map_err(|e| e.to_string())
}

/// Discards a buffered welcome without joining — the "no" side of the
/// invite-banner accept/decline pair `apps/desktop`'s
/// `AppCommand::DeclineGroupInvite` already models. Returns whether
/// there was actually a pending invite to discard (`false` isn't an
/// error — declining an already-decided invite is a no-op, not a
/// failure).
fn group_decline_invite_inner(conversation: &str) -> Result<bool, String> {
    let app = app_handle().ok_or("bootstrap must run before group_decline_invite")?;
    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let removed = app
        .pending_welcomes
        .lock()
        .expect("pending_welcomes lock poisoned")
        .remove(&conversation)
        .is_some();
    Ok(removed)
}

/// `MessageService::send_attachment` — see this crate's top doc comment
/// for why this is 1:1-only. `conversation` is generated fresh here
/// (`ConversationId::new()`), same as [`send_text_inner`] — this crate
/// has no multi-message-thread concept per peer yet (a real, existing
/// limitation this function inherits, not one it introduces).
fn send_attachment_inner(
    peer_ticket: &str,
    file_bytes: Vec<u8>,
    media_type_str: &str,
) -> Result<String, String> {
    let app = app_handle().ok_or("bootstrap must run before send_attachment")?;
    let peer = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let media_type = media_type_from_str(media_type_str);
    let message_id = runtime()
        .block_on(
            app.service
                .send_attachment(ConversationId::new(), &peer, file_bytes, media_type),
        )
        .map_err(|e| e.to_string())?;
    Ok(message_id.to_string())
}

/// `MessageService::fetch_attachment` — takes the exact fields the
/// `attachment\t...` poll-event line carried (see this module's top
/// doc comment for that format), reconstructs the `AttachmentReference`
/// it was built from, and returns the decrypted plaintext bytes.
fn fetch_attachment_inner(
    peer_ticket: &str,
    blob_hash_b64: &str,
    encrypted_size_bytes: u64,
    media_type_str: &str,
    attachment_key_b64: &str,
) -> Result<Vec<u8>, String> {
    let app = app_handle().ok_or("bootstrap must run before fetch_attachment")?;
    let peer = PeerTicket::decode(peer_ticket).map_err(|e| e.to_string())?;
    let blob_hash_vec = base64_decode(blob_hash_b64)?;
    let blob_hash: [u8; 32] = blob_hash_vec
        .try_into()
        .map_err(|_| "blob hash must be 32 bytes".to_string())?;
    let attachment_key_vec = base64_decode(attachment_key_b64)?;
    let attachment_key: [u8; 32] = attachment_key_vec
        .try_into()
        .map_err(|_| "attachment key must be 32 bytes".to_string())?;
    let reference = siar_domain::AttachmentReference {
        blob_hash,
        encrypted_size: siar_domain::BlobSize::parse(encrypted_size_bytes)
            .map_err(|e| e.to_string())?,
        media_type: media_type_from_str(media_type_str),
        attachment_key,
        thumbnail: None,
    };
    runtime()
        .block_on(app.service.fetch_attachment(&peer, &reference))
        .map_err(|e| e.to_string())
}

fn poll_next_event_inner() -> Option<String> {
    app_handle()?
        .events
        .lock()
        .expect("events lock poisoned")
        .pop_front()
}

/// The `mark_link_up` counterpart this crate's own doc comment named
/// as missing — reports `TransportLink::InternetDirect` down when
/// Kotlin calls it (intended for `MainActivity.onDestroy`/`onStop`).
///
/// Now a genuine teardown, not just a connectivity-state update: `APP`
/// holding `Mutex<Option<Arc<AppMessaging>>>` instead of a `OnceLock`
/// (see that static's own comment for why) means this can actually
/// take the app out of the slot, stop its pump task, and let its
/// `Arc<SiarEndpoint>` drop instead of leaking until process exit.
/// Concretely: `.take()` empties `APP` immediately, so every other
/// function in this file (`app_handle`) sees "not bootstrapped" from
/// this point on and a later `bootstrap` call genuinely builds a fresh
/// endpoint/service/pump rather than silently handing back the old
/// ticket. The pump is stopped via `JoinHandle::abort`, which
/// schedules cancellation rather than guaranteeing it lands
/// synchronously — Tokio drops an aborted task's captured state
/// (including its own `Arc<AppMessaging>` clone, the last thing
/// keeping the endpoint alive once this function's local `app` binding
/// also drops at the end of this function) at its own next scheduling
/// point, not necessarily before this function returns. That's a real
/// limitation worth naming, not a gap papered over: this function's
/// synchronous, verifiable guarantee is that `APP` is empty and the
/// pump has been told to stop the instant it returns; full resource
/// release (the endpoint's own `Drop`, whatever that does) follows
/// shortly after on the runtime's own schedule.
fn shutdown_inner() {
    let Some(app) = APP.lock().expect("APP lock poisoned").take() else {
        return;
    };
    if let Some(handle) = app
        .pump_handle
        .lock()
        .expect("pump_handle lock poisoned")
        .take()
    {
        handle.abort();
    }
    siar_android_connectivity::mark_link_down(siar_domain::TransportLink::InternetDirect);
    // `app` drops here — the slot's own strong reference to
    // `AppMessaging` (and, through it, `Arc<SiarEndpoint>`) is gone;
    // only the aborted pump task's clone remains until Tokio finishes
    // dropping that task, per the comment above.
}

#[cfg(target_os = "android")]
mod jni_bridge {
    use super::*;
    use jni::objects::{JClass, JObject, JString};
    use jni::sys::jstring;
    use jni::JNIEnv;

    /// Turns a `Result<String, String>`/`Option<String>` into a
    /// `jstring`, prefixing an `Err` with `"error:"` — the simplest
    /// possible error channel across this boundary (Kotlin checks
    /// `result.startsWith("error:")`) rather than a second JNI calling
    /// convention (exceptions, out-parameters) for the rare failure
    /// case. Consistent within this crate; not necessarily how a larger
    /// FFI surface should scale this pattern.
    fn to_jstring<'local>(env: &mut JNIEnv<'local>, value: Result<String, String>) -> jstring {
        let text = match value {
            Ok(s) => s,
            Err(e) => format!("error:{e}"),
        };
        env.new_string(text)
            .expect("failed to allocate a JNI string")
            .into_raw()
    }

    fn jstring_to_string<'local>(env: &mut JNIEnv<'local>, s: &JString<'local>) -> String {
        env.get_string(s).expect("invalid UTF-8 from Kotlin").into()
    }

    /// Real, previously-missing prerequisite for `bootstrap`/`SiarEndpoint::
    /// bind` to work reliably on Android — confirmed directly against
    /// iroh 1.0.3's real published docs (`docs.rs/iroh/1.0.3/iroh/
    /// endpoint/struct.Endpoint.html`'s own "Usage on Android" section,
    /// and `docs.rs/iroh-dns/1.0.3/iroh_dns/fn.install_android_jni_context.html`
    /// directly — not guessed): the endpoint's default `DnsResolver`
    /// reads Android's system DNS configuration through JNI, which
    /// needs a `JavaVM`/Application `Context` published to
    /// `ndk_context` *before* the endpoint is constructed. Without
    /// this, iroh falls back to Google's public DNS servers — and if
    /// this app's compilation profile ever sets `panic = "abort"`
    /// (not currently the case, but worth flagging), the fallback
    /// detection itself can't work and the app would panic instead.
    ///
    /// `JNI_OnLoad` is the standard, un-namespaced JNI entry point the
    /// JVM calls automatically the moment `System.loadLibrary
    /// ("siar_android_messaging")` finishes loading this `.so` — before
    /// any `Java_com_siar_messaging_...` function is ever called. This
    /// crate previously used it to also install iroh's Android DNS
    /// context, passing this function's own `reserved: *mut c_void`
    /// parameter through as if it were the Application `Context` —
    /// flagged at the time as an unverified guess copied from a docs.rs
    /// example. It wasn't just unverified, it was wrong: every real JNI
    /// reference checked this pass (Android's own NDK docs, multiple
    /// independent JNI_OnLoad examples, and a library maintainer's own
    /// answer to this exact "how do I get an Android Context to a
    /// native lib" problem for a different crate) agrees the JNI
    /// spec's `reserved` parameter is exactly that — reserved, unused,
    /// always null in practice — never a `Context`. That guess is
    /// removed; [`initAndroidContext`] below is the real fix. This
    /// function now does only what `JNI_OnLoad` is actually for:
    /// confirming this library supports the JNI version the JVM is
    /// asking for.
    ///
    /// (One consequence worth naming rather than hiding: since iroh
    /// 1.0.1, a missing/invalid Android DNS context is no longer fatal
    /// — iroh falls back to Google's public DNS servers instead of
    /// panicking, confirmed via iroh's own 1.0.1 release notes. So the
    /// previous wrong guess likely never crashed this app; it just
    /// silently never worked, always falling back. [`initAndroidContext`]
    /// is what makes it actually work.)
    #[no_mangle]
    pub extern "C" fn JNI_OnLoad(
        _vm: jni::JavaVM,
        _reserved: *mut std::ffi::c_void,
    ) -> jni::sys::jint {
        jni::JNIVersion::V6.into()
    }

    /// The real fix for what `JNI_OnLoad`'s own doc comment above used
    /// to guess at: an explicit entry point Kotlin calls once, early —
    /// `MainActivity.onCreate`, before [`bootstrap`] — with the real
    /// `Context` it already has (`applicationContext`), matching the
    /// standard fix a `uniffi-rs` maintainer gave for this exact
    /// "native lib needs an Android Context but has no reliable way to
    /// get one" problem: "expose a function that initializes the
    /// Android context for you and make your app/library/kotlin
    /// wrapper call that as the first thing."
    ///
    /// The `Context` object a JNI call hands this function is only
    /// valid as a *local* reference — guaranteed good for the duration
    /// of this one call, not beyond it — but `install_android_jni_context`
    /// stores the pointer for reuse by DNS lookups that happen long
    /// after this function returns. So it's promoted to a JNI *global*
    /// reference first (`new_global_ref`), the standard fix for "this
    /// needs to outlive the call that handed it to me," same reasoning
    /// `ndk-context`'s own `AndroidContext` type stores a global rather
    /// than local reference internally. That global ref is then
    /// deliberately leaked (`std::mem::forget`), not released: this
    /// process has exactly one Application `Context` for its entire
    /// lifetime, so there is nothing to ever release it *to* — same
    /// one-global-ref-per-process-lifetime shape `ndk-context` itself
    /// uses. Same deliberate leak for the `JavaVM` handle, for the same
    /// reason. The exact `new_global_ref`/`get_java_vm`/
    /// `as_obj().as_raw()` shape below isn't a guess: it matches a
    /// real, confirmed-working example (flutter_rust_bridge's own
    /// `ndk-init` integration guide) that pins the identical `jni =
    /// "0.21"` version this crate uses for this exact
    /// Context-to-native-code problem, not this pass's own invention.
    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_initAndroidContext<
        'local,
    >(
        env: JNIEnv<'local>,
        _class: JClass<'local>,
        context: JObject<'local>,
    ) {
        let global_context = match env.new_global_ref(&context) {
            Ok(g) => g,
            Err(e) => {
                tracing::error!(error = %e, "initAndroidContext: failed to create a global ref for the Context");
                return;
            }
        };
        let java_vm = match env.get_java_vm() {
            Ok(vm) => vm,
            Err(e) => {
                tracing::error!(error = %e, "initAndroidContext: failed to obtain the JavaVM handle");
                return;
            }
        };
        let context_ptr = global_context.as_obj().as_raw() as *mut std::ffi::c_void;
        let vm_ptr = java_vm.get_java_vm_pointer() as *mut std::ffi::c_void;
        std::mem::forget(global_context); // deliberately leaked — see this function's own doc comment
        std::mem::forget(java_vm); // deliberately leaked — see this function's own doc comment
        unsafe {
            iroh::dns::install_android_jni_context(vm_ptr, context_ptr);
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_bootstrap<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        base_dir: JString<'local>,
    ) -> jstring {
        let base_dir = jstring_to_string(&mut env, &base_dir);
        let result = bootstrap_inner(&base_dir);
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_ticketEndpointDebug<
        'local,
    >(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        ticket: JString<'local>,
    ) -> jstring {
        let ticket = jstring_to_string(&mut env, &ticket);
        let result = ticket_endpoint_debug_inner(&ticket);
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_addPeer<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        ticket: JString<'local>,
    ) -> jstring {
        let ticket = jstring_to_string(&mut env, &ticket);
        let result = add_peer_inner(&ticket).map(|()| "ok".to_string());
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_sendText<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        peer_ticket: JString<'local>,
        text: JString<'local>,
    ) -> jstring {
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let text = jstring_to_string(&mut env, &text);
        let result = send_text_inner(&peer_ticket, &text);
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_checkMailbox<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        relay_ticket: JString<'local>,
    ) -> jstring {
        let relay_ticket = jstring_to_string(&mut env, &relay_ticket);
        let result = check_mailbox_inner(&relay_ticket).map(|()| "ok".to_string());
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_sendTextAnon<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        peer_ticket: JString<'local>,
        relay_ticket: JString<'local>,
        text: JString<'local>,
    ) -> jstring {
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let relay_ticket = jstring_to_string(&mut env, &relay_ticket);
        let text = jstring_to_string(&mut env, &text);
        let result = send_text_anon_inner(&peer_ticket, &relay_ticket, &text);
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_checkMailboxAnon<
        'local,
    >(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        peer_ticket: JString<'local>,
        relay_ticket: JString<'local>,
    ) -> jstring {
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let relay_ticket = jstring_to_string(&mut env, &relay_ticket);
        let result =
            check_mailbox_anon_inner(&peer_ticket, &relay_ticket).map(|()| "ok".to_string());
        to_jstring(&mut env, result)
    }

    /// Returns `null` (not an empty string — Kotlin's `external fun`
    /// declares this nullable) when nothing is queued, so a caller
    /// polling on a timer can distinguish "nothing yet" from a real
    /// empty-string event, which none of today's two event kinds would
    /// ever produce anyway but a future one might.
    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_pollNextEvent<'local>(
        env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        match poll_next_event_inner() {
            Some(line) => env
                .new_string(line)
                .expect("failed to allocate a JNI string")
                .into_raw(),
            None => std::ptr::null_mut(),
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_shutdown<'local>(
        _env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) {
        shutdown_inner();
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_deviceId<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        let result = device_id_inner();
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_accountId<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        let result = account_id_inner();
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupKeyPackage<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        let result = group_key_package_inner();
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupCreate<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
    ) -> jstring {
        let result = group_create_inner();
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupAddMember<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        conversation: JString<'local>,
        peer_ticket: JString<'local>,
        peer_device_id: JString<'local>,
        peer_account_id: JString<'local>,
        key_package_b64: JString<'local>,
    ) -> jstring {
        let conversation = jstring_to_string(&mut env, &conversation);
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let peer_device_id = jstring_to_string(&mut env, &peer_device_id);
        let peer_account_id = jstring_to_string(&mut env, &peer_account_id);
        let key_package_b64 = jstring_to_string(&mut env, &key_package_b64);
        let result = group_add_member_inner(
            &conversation,
            &peer_ticket,
            &peer_device_id,
            &peer_account_id,
            &key_package_b64,
        );
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupSendText<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        conversation: JString<'local>,
        text: JString<'local>,
    ) -> jstring {
        let conversation = jstring_to_string(&mut env, &conversation);
        let text = jstring_to_string(&mut env, &text);
        let result = group_send_text_inner(&conversation, &text);
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupJoin<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        conversation: JString<'local>,
        group_state_b64: JString<'local>,
    ) -> jstring {
        let conversation = jstring_to_string(&mut env, &conversation);
        let group_state_b64 = jstring_to_string(&mut env, &group_state_b64);
        let result = group_join_inner(&conversation, &group_state_b64).map(|()| "ok".to_string());
        to_jstring(&mut env, result)
    }

    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_groupDeclineInvite<
        'local,
    >(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        conversation: JString<'local>,
    ) -> jstring {
        let conversation = jstring_to_string(&mut env, &conversation);
        // No natural `jboolean` return without a second marshalling
        // convention alongside `to_jstring`'s `"error:"`-prefixed
        // string one — kept to that one existing convention instead:
        // `"true"`/`"false"` as plain text, same as every other
        // success value this crate returns across JNI.
        let result = group_decline_invite_inner(&conversation).map(|removed| removed.to_string());
        to_jstring(&mut env, result)
    }

    /// `file_bytes_b64`, not a `jbyteArray` parameter — see this
    /// module's own note on why attachment bytes move as base64 text
    /// through the same `to_jstring`/`"error:"` convention as
    /// everything else in this crate rather than introducing a second,
    /// separately-unverified raw-byte-array JNI marshalling path.
    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_sendAttachment<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        peer_ticket: JString<'local>,
        file_bytes_b64: JString<'local>,
        media_type: JString<'local>,
    ) -> jstring {
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let file_bytes_b64 = jstring_to_string(&mut env, &file_bytes_b64);
        let media_type = jstring_to_string(&mut env, &media_type);
        let result = base64_decode(&file_bytes_b64)
            .and_then(|file_bytes| send_attachment_inner(&peer_ticket, file_bytes, &media_type));
        to_jstring(&mut env, result)
    }

    /// Returns the decrypted plaintext as base64 text on success — same
    /// reasoning as [`sendAttachment`] above. `"error:"`-prefixed on
    /// failure, same as every other call in this crate; Kotlin decodes
    /// with `android.util.Base64` on the success path.
    #[no_mangle]
    pub extern "system" fn Java_com_siar_messaging_NativeMessagingBridge_fetchAttachment<'local>(
        mut env: JNIEnv<'local>,
        _class: JClass<'local>,
        peer_ticket: JString<'local>,
        blob_hash_b64: JString<'local>,
        encrypted_size_bytes: jni::sys::jlong,
        media_type: JString<'local>,
        attachment_key_b64: JString<'local>,
    ) -> jstring {
        let peer_ticket = jstring_to_string(&mut env, &peer_ticket);
        let blob_hash_b64 = jstring_to_string(&mut env, &blob_hash_b64);
        let media_type = jstring_to_string(&mut env, &media_type);
        let attachment_key_b64 = jstring_to_string(&mut env, &attachment_key_b64);
        // `jlong` (Kotlin `Long`) rather than an unsigned type — JNI/
        // Kotlin have no unsigned integer primitive that crosses this
        // boundary cleanly (`jni` 0.21 exposes signed types only, same
        // as every other numeric parameter anywhere in this crate,
        // which has none until this one) — cast to `u64` here since a
        // real attachment size is never negative and `MAX_ATTACHMENT_BYTES`
        // (200 MiB) is far below `i64::MAX` regardless.
        let encrypted_size_bytes = encrypted_size_bytes as u64;
        let result = fetch_attachment_inner(
            &peer_ticket,
            &blob_hash_b64,
            encrypted_size_bytes,
            &media_type,
            &attachment_key_b64,
        )
        .map(|bytes| base64_encode(&bytes));
        to_jstring(&mut env, result)
    }
}
