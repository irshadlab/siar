//! Phase-1/2 acceptance harness (plan.md §98, §124): two independent
//! `siar` processes, Alice and Bob, exchanging pairing tickets by hand
//! and then sending real, retried, ACKed E2EE text messages over iroh.
//!
//! Also the first thing in this workspace that actually constructs a
//! `GroupService` (`group-create`/`group-add-member`/`group-send`/
//! `join-group`/`publish-key-package`/`listen --publish-key-package`)
//! — every piece of the MLS group path existed as library code before
//! this; nothing had ever wired it into a running process. Same
//! hand-paired, copy-paste-between-terminals spirit as `listen`/`send`
//! below: a group's founder runs `group-create`, each intended member
//! runs `listen --publish-key-package [their-ticket]` (staying in that
//! one process — see below for why) and pastes the printed device id /
//! account id / ticket / key package back to the founder, who runs
//! `group-add-member` once per member (also printing the group's
//! current state). Whoever's `listen --publish-key-package` receives
//! the resulting `GroupMlsWelcome` gets prompted right there in that
//! same terminal to paste the printed group state and accept the
//! invitation — the welcome *bytes* themselves need no copy-paste, they
//! arrive over the wire in the envelope.
//!
//! Standalone `join-group` is still here too, but — flagged plainly,
//! not left to be discovered by trying it — **it cannot succeed run as
//! its own separate process.** `join_group_mls` needs
//! `GroupService`'s `pending_identity` — the key material
//! `publish_key_package` generated — and that field doesn't survive a
//! process exit (see `pending_identity`'s own doc comment). A bare
//! `publish-key-package` command exits immediately after printing, so
//! anything that later runs `join-group` as a fresh process always
//! finds `pending_identity` empty (`NoPendingKeyPackageIdentity`).
//! `listen --publish-key-package`'s in-loop prompt above is the one
//! that actually works, because publishing and joining share the same
//! process/`GroupService` instance. `join-group` the standalone command
//! is kept because its logic and call shape are still correct — useful
//! for scripting `join_group_mls` directly against welcome bytes
//! obtained some other way — just not as a "run this after
//! publish-key-package" two-step today.
//!
//! Usage:
//!   siar listen [--publish-key-package] [their-ticket]
//!   siar send <their-ticket> <text>   # sends one message and exits
//!   siar send-file <their-ticket> <path>
//!   siar publish-key-package
//!   siar check-mailbox <relay-ticket>
//!   siar group-create
//!   siar group-add-member <conversation-id> <peer-ticket> <peer-device-id> <peer-account-id> <base64-key-package>
//!   siar group-send <conversation-id> <text>
//!   siar join-group <conversation-id> <base64-welcome-bytes> <base64-group-state>   # see note above — not independently usable yet
//!
//! `listen` takes the peer's ticket up front so it knows which X25519 key
//! to decrypt incoming frames with — see ticket.rs's module docs for why
//! this hand-pairing is Phase-1-only (real contact discovery is
//! plan.md §41–42, a later phase).

mod args;

use anyhow::{Context, Result};
use args::{Args, Command};
use clap::Parser as _;
use siar_crypto::DeviceIdentity;
use siar_domain::{AccountId, ConversationId, DeviceId, MessageContent, MessageText};
use siar_messaging::{
    GroupService, InMemoryDeviceDirectory, InMemoryKeyPackageDirectory, IncomingEvent,
    KeyPackageDirectory, MemberDevice, MessageService, PeerTicket,
};
use siar_protocol::v1::EnvelopeKind;
use siar_protocol::WireMessage;
use siar_transport::{IncomingFrame, PeerTransport, SiarEndpoint};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    match Args::parse().command {
        Command::Listen {
            publish_key_package,
            their_ticket,
        } => listen(their_ticket.as_ref(), publish_key_package).await,
        Command::Send { their_ticket, text } => send(&their_ticket, &text).await,
        Command::SendFile { their_ticket, path } => send_file(&their_ticket, &path).await,
        Command::PublishKeyPackage => publish_key_package().await,
        Command::CheckMailbox { relay_ticket } => check_mailbox(&relay_ticket).await,
        Command::SendAnon {
            their_ticket,
            relay_ticket,
            text,
        } => send_anon(&their_ticket, &relay_ticket, &text).await,
        Command::CheckMailboxAnon {
            their_ticket,
            relay_ticket,
        } => check_mailbox_anon(&their_ticket, &relay_ticket).await,
        Command::GroupCreate => group_create().await,
        Command::GroupAddMember {
            conversation_id,
            peer_ticket,
            peer_device_id,
            peer_account_id,
            base64_key_package,
        } => {
            group_add_member(
                &conversation_id,
                &peer_ticket,
                &peer_device_id,
                &peer_account_id,
                &base64_key_package,
            )
            .await
        }
        Command::GroupSend {
            conversation_id,
            text,
        } => group_send(&conversation_id, &text).await,
        Command::JoinGroup {
            conversation_id,
            base64_welcome_bytes,
            base64_group_state,
        } => join_group(&conversation_id, &base64_welcome_bytes, &base64_group_state).await,
    }
}

/// Everything `bootstrap()` assembles for one process — a small struct
/// instead of an ever-growing tuple now that group commands need
/// several more pieces than `MessageService` alone did.
struct Bootstrapped {
    endpoint: Arc<SiarEndpoint>,
    device_id: DeviceId,
    service: MessageService,
    group_service: GroupService,
    device_directory: Arc<InMemoryDeviceDirectory>,
    key_package_directory: Arc<InMemoryKeyPackageDirectory>,
    local_account: AccountId,
    my_ticket: PeerTicket,
    rx: mpsc::Receiver<IncomingFrame>,
}

/// Common setup for both modes (plan.md §84's staged startup).
///
/// Identity/account id/device id/database now persist under an
/// OS-appropriate data directory (`directories::ProjectDirs`) —
/// closing the gap this file's own doc comment used to name as a
/// known Phase-1 stand-in ("no persisted identity yet"). Same exact
/// pattern `apps/desktop`'s `resolve_data_paths`/`load_or_create_id`
/// already used and `apps/android/messaging-jni`'s `AppDataPaths`
/// mirrored for Android — this is the third and last of this
/// workspace's three client entry points to gain it. All three now
/// agree on the shape (bare UUID text files for ids,
/// `DeviceIdentity::save_to_file`/`load_from_file` for the key
/// material, a real on-disk `siar_storage::open` database) though
/// each resolves its own OS-appropriate directory independently —
/// sharing the actual struct/function across three separate binaries
/// would mean a new shared crate for ~30 lines, not worth the
/// coupling for this pass.
struct DataPaths {
    identity: std::path::PathBuf,
    account_id: std::path::PathBuf,
    device_id: std::path::PathBuf,
    database: std::path::PathBuf,
}

fn resolve_data_paths() -> Result<DataPaths> {
    let dirs = directories::ProjectDirs::from("dev", "irshad", "siar-cli")
        .context("couldn't resolve a data directory for this platform")?;
    let data_dir = dirs.data_dir();
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;
    Ok(DataPaths {
        identity: data_dir.join("identity.bin"),
        account_id: data_dir.join("account_id.txt"),
        device_id: data_dir.join("device_id.txt"),
        database: data_dir.join("siar.db"),
    })
}

/// Loads an id previously written by this function, or generates and
/// persists a fresh one on first run — identical to `apps/desktop`'s
/// own `load_or_create_id` (see that function's doc comment for why a
/// bare UUID string, not postcard/JSON).
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

async fn bootstrap() -> Result<Bootstrapped> {
    let paths = resolve_data_paths()?;

    let identity = if paths.identity.exists() {
        DeviceIdentity::load_from_file(&paths.identity)
            .context("loading persisted device identity")?
    } else {
        let identity = DeviceIdentity::generate();
        identity
            .save_to_file(&paths.identity)
            .context("persisting device identity")?;
        identity
    };
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

    let db = siar_storage::open(&paths.database.display().to_string())
        .context("opening local database")?;
    let messages = Arc::new(siar_storage::StoolapMessageRepository::new(db.clone()));
    let outbox = Arc::new(siar_storage::StoolapOutboxRepository::new(db.clone()));
    let groups = Arc::new(siar_storage::StoolapGroupRepository::new(db.clone()));
    let blobs: Arc<dyn siar_storage::BlobRepository + Send + Sync> =
        Arc::new(siar_storage::StoolapBlobRepository::new(db));
    let blob_store: Arc<dyn siar_transport::BlobStore> =
        Arc::new(siar_messaging::StorageBlobStore(blobs.clone()));

    let (tx, rx) = mpsc::channel::<IncomingFrame>(64);
    // iroh-base 1.0.3's `SecretKey::generate()` takes no RNG argument —
    // it uses an internal CSPRNG itself now (0.95.1 took `&mut OsRng`;
    // that's the one other real API shape change this iroh bump
    // introduced, alongside `Endpoint::builder(preset)` from earlier).
    let iroh_secret = iroh::SecretKey::generate();
    let endpoint = Arc::new(SiarEndpoint::bind(iroh_secret, tx, blob_store).await?);

    let my_ticket = PeerTicket {
        endpoint_addr: endpoint.addr(),
        x25519_public: identity.x25519_public().to_bytes(),
        ed25519_verifying: identity.verifying_key().to_bytes(),
    };

    let device_directory = Arc::new(InMemoryDeviceDirectory::new());
    let key_package_directory = Arc::new(InMemoryKeyPackageDirectory::new());

    // `MessageService` and `GroupService` both take a `DeviceIdentity`
    // by value, and both represent this same local device — `try_clone`
    // (see its own doc comment) is exactly the "more than one owner of
    // the same key material in one process" case it exists for.
    let group_identity = identity
        .try_clone()
        .context("cloning device identity for GroupService")?;
    let group_service = GroupService::new(
        device_id,
        local_account,
        group_identity,
        endpoint.clone(),
        messages.clone(),
        device_directory.clone(),
        groups,
    );

    let service = MessageService::new(
        device_id,
        identity,
        endpoint.clone(),
        messages,
        outbox,
        blobs,
    );

    Ok(Bootstrapped {
        endpoint,
        device_id,
        service,
        group_service,
        device_directory,
        key_package_directory,
        local_account,
        my_ticket,
        rx,
    })
}

/// plan.md §33's retry scheduler, run as a background task on a fixed
/// cadence. One second is plenty for a two-peer CLI test — a real client
/// would tie this to network-state-change events too (plan.md §33's
/// "immediately retry when network state changes from offline -> online"),
/// which needs `iroh::Endpoint::network_change()`, left for the desktop
/// app's lifecycle-aware sync (plan.md §36).
fn spawn_retry_scheduler(service: Arc<MessageService>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            if let Err(e) = service.retry_due().await {
                tracing::warn!(error = %e, "retry_due failed");
            }
        }
    });
}

async fn listen(peer_arg: Option<&String>, publish_key_package: bool) -> Result<()> {
    let boot = bootstrap().await?;
    let service = Arc::new(boot.service);
    let group_service = Arc::new(boot.group_service);
    let key_package_directory = boot.key_package_directory;
    let device_id = boot.device_id;
    println!(
        "your ticket (share with your peer):\n{}",
        boot.my_ticket.encode()
    );
    println!(
        "your account id (share for group membership): {}",
        boot.local_account
    );
    println!(
        "your device id (share for group membership): {}",
        boot.device_id
    );

    // The fix for this file's top doc comment's flagged limitation:
    // `pending_identity` only lives as long as this process does, so
    // publishing and (later, in this same loop) joining now happen in
    // one long-running `listen` invocation instead of two separate
    // one-shot commands. `join-group` as its own command is still
    // useful for testing/scripting `join_group_mls` directly, but this
    // is the flow that actually works end-to-end.
    if publish_key_package {
        group_service.publish_key_package(key_package_directory.as_ref())?;
        let key_package_bytes = key_package_directory
            .take(device_id)
            .context("just-published key package vanished")?;
        println!(
            "published a key package for group invitations — give the group admin:\n  \
             device id: {device_id}\n  \
             account id: {}\n  \
             ticket: {}\n  \
             key package (base64): {}",
            boot.local_account,
            boot.my_ticket.encode(),
            base64_encode(&key_package_bytes),
        );
    }

    let peer = match peer_arg {
        Some(t) => Some(PeerTicket::decode(t).context("decoding peer ticket")?),
        None => {
            println!("(no peer ticket given — incoming frames will be dropped until one is)");
            None
        }
    };

    spawn_retry_scheduler(service.clone());

    println!("listening...");
    let mut rx = boot.rx;
    let mut stdin_lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Some(frame) = rx.recv().await {
        tracing::info!(from = %frame.from, "listen loop received IncomingFrame from rx");
        let envelope = match frame.message {
            WireMessage::V1(envelope) => envelope,
            WireMessage::Mesh(_mesh_envelope) => {
                // next.md §29's relay-routable envelope — this CLI has
                // no DTN forwarding/relay logic wired up (that's
                // `apps/emergency-node`'s job), so there's nothing
                // useful to do with one here yet beyond not crashing on
                // it, which the old `let WireMessage::V1(envelope) =
                // frame.message;` irrefutable pattern would have done
                // the moment a `Mesh` frame arrived.
                println!(
                    "(dropping a Mesh-routed frame from {} — this CLI doesn't relay)",
                    frame.from
                );
                continue;
            }
            WireMessage::MailboxCheckIn(_check_in) => {
                // next.md §76–77's mailbox check-in (see
                // `siar-protocol::mailbox`'s doc comment) — answering
                // one is a relay's job (`apps/emergency-node` does
                // this), not an ordinary chat client's. Same "don't
                // crash, don't pretend to handle it" stance as the
                // `Mesh` arm above.
                println!(
                    "(dropping a MailboxCheckIn frame from {} — this CLI isn't a relay)",
                    frame.from
                );
                continue;
            }
            WireMessage::TokenMailboxDeposit(_) | WireMessage::AnonymousMailboxCheckIn(_) => {
                // The unlinkable counterparts to `Mesh`/`MailboxCheckIn`
                // — same "a relay's job, not this CLI's" reasoning as
                // the two arms above.
                println!(
                    "(dropping a token-mailbox frame from {} — this CLI isn't a relay)",
                    frame.from
                );
                continue;
            }
            WireMessage::RouteAdvertisement(_advertisement) => {
                // A relay-to-relay signal (see `route_advertisement.rs`'s
                // doc comment) — this CLI has no `PathTable` of its own
                // to fold one into.
                println!(
                    "(dropping a RouteAdvertisement frame from {} — this CLI isn't a relay)",
                    frame.from
                );
                continue;
            }
        };

        // Group frames (`GroupEvent`/`GroupMls*`) go to `GroupService`
        // — `MessageService::handle_incoming` explicitly doesn't handle
        // these (see `IncomingEvent`'s doc comment), so check the kind
        // before falling through to the 1:1 path below.
        if matches!(
            envelope.kind,
            EnvelopeKind::GroupEvent
                | EnvelopeKind::GroupMlsCommit
                | EnvelopeKind::GroupMlsWelcome
                | EnvelopeKind::GroupMlsApplication
        ) {
            match envelope.kind {
                EnvelopeKind::GroupMlsWelcome => {
                    // The welcome *bytes* travelled here for free in
                    // `envelope.payload` — `add_member_mls` already
                    // sends them over the wire. What's still missing is
                    // the group's `GroupState` bookkeeping (next.md
                    // §41's real membership-sync problem, not solved
                    // here — see `join_group_mls`'s doc comment): the
                    // wire protocol has no envelope kind carrying that
                    // today, so it's a manual paste, same as
                    // `group-add-member`'s printed output was designed
                    // for.
                    if !publish_key_package {
                        println!(
                            "{}: [MLS welcome for conversation {} arrived, but this process wasn't started with \
                             `--publish-key-package`, so it has no pending identity to join with — ignoring]",
                            frame.from.fmt_short(),
                            envelope.conversation_id,
                        );
                        continue;
                    }
                    println!(
                        "{}: [MLS welcome for conversation {} arrived — paste the base64 group state the admin's \
                         `group-add-member` printed, or press Enter to ignore this invitation]",
                        frame.from.fmt_short(),
                        envelope.conversation_id,
                    );
                    let Some(Ok(line)) = stdin_lines.next_line().await.transpose() else {
                        println!("(stdin closed or unreadable — ignoring this invitation)");
                        continue;
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        println!("(ignored)");
                        continue;
                    }
                    match base64_decode(line).and_then(|b| {
                        postcard::from_bytes::<siar_domain::GroupState>(&b)
                            .context("decoding group state")
                    }) {
                        Ok(state) => match group_service.join_group_mls(
                            envelope.conversation_id,
                            &envelope.payload,
                            state,
                        ) {
                            Ok(()) => println!("joined group {}", envelope.conversation_id),
                            Err(e) => println!("(failed to join: {e})"),
                        },
                        Err(e) => println!("(that didn't decode as a group state: {e})"),
                    }
                }
                _ => match group_service.handle_incoming_mls(envelope.conversation_id, &envelope) {
                    Ok(Some(MessageContent::Text(text))) => {
                        println!("{} [group]: {}", frame.from.fmt_short(), text.as_str())
                    }
                    Ok(Some(_)) => {
                        println!("{} [group]: [non-text content]", frame.from.fmt_short())
                    }
                    Ok(None) => {} // commit merged, or nothing to show
                    Err(e) => tracing::warn!(error = %e, "failed to handle incoming group frame"),
                },
            }
            continue;
        }

        let Some(peer) = peer.as_ref() else {
            println!(
                "(dropping frame from {} — no peer ticket configured)",
                frame.from
            );
            continue;
        };
        match service.handle_incoming(peer, envelope).await {
            Ok(Some(IncomingEvent::Content(MessageContent::Text(text)))) => {
                println!("{}: {}", frame.from.fmt_short(), text.as_str())
            }
            Ok(Some(IncomingEvent::Content(MessageContent::Attachment(reference)))) => {
                // plan.md §65: don't auto-download — this just announces
                // the attachment arrived; fetching the bytes is a
                // separate, on-demand `service.fetch_attachment` call
                // this CLI doesn't wire up to a command yet.
                println!(
                    "{}: [attachment, {} bytes, hash {}]",
                    frame.from.fmt_short(),
                    reference.encrypted_size.bytes(),
                    hex_preview(&reference.blob_hash),
                );
            }
            Ok(Some(IncomingEvent::CallSignal { from, event })) => {
                println!("{}: [call signal {:?}]", from.fmt_short(), event);
            }
            Ok(None) => {
                tracing::info!("handle_incoming returned Ok(None) — duplicate, ack, or receipt")
            }
            Err(e) => tracing::warn!(error = %e, "failed to handle incoming frame"),
        }
    }

    Ok(())
}

async fn send(peer_ticket: &str, text: &str) -> Result<()> {
    let peer = PeerTicket::decode(peer_ticket).context("decoding peer ticket")?;
    let boot = bootstrap().await?;
    // We're a one-shot sender — nothing will read `rx`. Dropping it is
    // fine: `siar-transport`'s handler already treats a closed inbound
    // channel as "drop the connection", not a panic (plan.md §56
    // backpressure). That also means we won't see the peer's DeliveryAck
    // even if it comes back — a real one-shot CLI would keep `rx` alive
    // and wait briefly for the ack before exiting; left simple here.
    drop(boot.rx);

    let text = MessageText::parse(text.to_string()).context("message text")?;
    // Phase-1 stand-in: a real client looks up (or creates) the
    // conversation with this peer rather than minting a fresh one per
    // send — conversation persistence lands with siar-storage's
    // `conversations` table in a later phase.
    let conversation = ConversationId::new();

    let message_id = boot.service.send_text(conversation, &peer, text).await?;
    println!("sent {message_id}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn send_file(peer_ticket: &str, path: &str) -> Result<()> {
    let peer = PeerTicket::decode(peer_ticket).context("decoding peer ticket")?;
    let boot = bootstrap().await?;
    drop(boot.rx); // see send()'s comment on why this is fine for a one-shot CLI

    let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let media_type = guess_media_type(path);
    let conversation = ConversationId::new();

    let message_id = boot
        .service
        .send_attachment(conversation, &peer, bytes, media_type)
        .await?;
    println!("sent attachment {message_id}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Generates and publishes this device's key package, printing
/// everything a group founder needs to `group-add-member` it: this
/// process's device id, account id, ticket (so the founder can register
/// this device in *their* `InMemoryDeviceDirectory` before sending the
/// welcome), and the key package bytes themselves (base64, since this
/// Phase-1 CLI has no real `KeyPackageDirectory` shared between
/// processes — see `key_package_directory.rs`'s own doc comment on
/// scope — so the bytes have to move the same copy-paste way
/// `PeerTicket`s already do).
///
/// This process then exits — it isn't the one calling
/// `join_group_mls`, so it doesn't need to stay running. A real client
/// would keep the identity `publish_key_package` generated alive
/// (`GroupService`'s `pending_identity`) across process restarts, which
/// needs identity persistence this Phase-1 harness doesn't have yet
/// (see `DeviceIdentity::save_to_file`'s own scope note) — so this
/// one-shot command is honestly only a demonstration of the publish
/// half, not a fully persistent flow.
async fn publish_key_package() -> Result<()> {
    let boot = bootstrap().await?;
    drop(boot.rx);

    boot.group_service
        .publish_key_package(boot.key_package_directory.as_ref())?;
    let key_package_bytes = boot
        .key_package_directory
        .take(boot.device_id)
        .context("just-published key package vanished")?;

    println!("device id: {}", boot.device_id);
    println!("account id: {}", boot.local_account);
    println!("ticket: {}", boot.my_ticket.encode());
    println!(
        "key package (base64): {}",
        base64_encode(&key_package_bytes)
    );
    Ok(())
}

/// Sends a `MailboxCheckIn` (see `siar-protocol::mailbox`'s doc
/// comment) to a relay and prints whatever `MeshEnvelope`s come back
/// within a short window. This is the client-side half `apps/emergency-
/// node`'s mailbox handler needed — before this, nothing anywhere
/// actually sent one.
///
/// Deliberately stops at "print that something arrived," not "decrypt
/// and show the message." A `MeshEnvelope`'s `ciphertext` is normally
/// an encoded `WireMessage::V1(Envelope)` from a real end-to-end
/// session between the original sender and this device (see
/// `siar-protocol::mesh`'s doc comment) — decrypting it needs that
/// original sender's `PeerTicket`, looked up by the *inner* envelope's
/// `sender: DeviceId` once decoded. This CLI has no sender-to-ticket
/// directory to do that lookup with (its `InMemoryDeviceDirectory` is
/// keyed by `AccountId`, one level up, and only populated for group
/// membership so far) — real follow-up work, not attempted here rather
/// than guessed at with whatever ticket happened to be lying around.
async fn check_mailbox(relay_ticket: &str) -> Result<()> {
    let relay = PeerTicket::decode(relay_ticket).context("decoding relay ticket")?;
    let boot = bootstrap().await?;
    let mut rx = boot.rx;

    boot.endpoint
        .send(
            relay.endpoint_addr.clone(),
            &WireMessage::MailboxCheckIn(
                boot.service
                    .sign_mailbox_check_in(siar_domain::now_millis()),
            ),
        )
        .await
        .context("sending mailbox check-in")?;
    println!(
        "check-in sent for device {} — waiting up to 5s for a response...",
        boot.device_id
    );

    let mut received = 0u32;
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                match frame.message {
                    WireMessage::Mesh(envelope) => {
                        received += 1;
                        println!(
                            "  mailbox item {}: {} bytes, hop_limit {}, priority {:?} (encrypted — see this fn's doc comment on why this CLI can't decode it yet)",
                            received,
                            envelope.ciphertext.len(),
                            envelope.hop_limit,
                            envelope.priority,
                        );
                    }
                    _ => tracing::debug!(from = ?frame.from, "ignoring a non-Mesh frame while waiting on mailbox responses"),
                }
            }
        }
    }

    println!("done — received {received} mailbox item(s)");
    Ok(())
}

/// The unlinkable counterpart to [`send`] — see `MessageService::
/// send_text_anon`'s own doc comment for exactly what this does and
/// doesn't guarantee (no delivery ack/retry on this path, in
/// particular). `peer_ticket` here is who the message is *for*;
/// `relay_ticket` is who it's handed to for pickup — genuinely two
/// different peers in the anonymous-path model, unlike `check_mailbox`
/// where the relay ticket is the only one needed.
async fn send_anon(peer_ticket: &str, relay_ticket: &str, text: &str) -> Result<()> {
    let peer = PeerTicket::decode(peer_ticket).context("decoding peer ticket")?;
    let relay = PeerTicket::decode(relay_ticket).context("decoding relay ticket")?;
    let boot = bootstrap().await?;
    drop(boot.rx); // see send()'s comment on why this is fine for a one-shot CLI

    let text = MessageText::parse(text.to_string()).context("message text")?;
    let message_id = boot.service.send_text_anon(&peer, &relay, text).await?;
    println!("sent {message_id} via the anonymous token-mailbox path");
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// The unlinkable counterpart to [`check_mailbox`] — presents a
/// [`siar_crypto::mailbox_token::MailboxToken`] instead of this
/// device's own `DeviceId` (see `siar_crypto::mailbox_token`'s doc
/// comment for the unlinkability property this buys and its real
/// limits), and can actually decrypt what comes back — unlike
/// `check_mailbox`'s `Mesh`-envelope items, an `AnonymousMailboxCheckIn`
/// response's sender is never in question: it's always `peer_ticket`,
/// since that's the only identity this token could have been derived
/// against.
async fn check_mailbox_anon(peer_ticket: &str, relay_ticket: &str) -> Result<()> {
    let peer = PeerTicket::decode(peer_ticket).context("decoding peer ticket")?;
    let relay = PeerTicket::decode(relay_ticket).context("decoding relay ticket")?;
    let boot = bootstrap().await?;
    let mut rx = boot.rx;

    let check_in = boot
        .service
        .build_anonymous_check_in(&peer, siar_domain::now_millis());

    boot.endpoint
        .send(
            relay.endpoint_addr.clone(),
            &WireMessage::AnonymousMailboxCheckIn(check_in),
        )
        .await
        .context("sending anonymous mailbox check-in")?;
    println!("anonymous check-in sent — waiting up to 5s for a response...");

    let mut received = 0u32;
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                match frame.message {
                    WireMessage::TokenMailboxDeposit(envelope) => {
                        received += 1;
                        match boot.service.decrypt_token_mailbox_envelope(&peer, &envelope) {
                            Ok(MessageContent::Text(text)) => {
                                println!("  mailbox item {}: \"{}\"", received, text.as_str());
                            }
                            Ok(MessageContent::Attachment(reference)) => {
                                println!("  mailbox item {}: attachment, {} bytes", received, reference.encrypted_size.bytes());
                            }
                            Err(e) => println!("  mailbox item {received}: couldn't decrypt ({e}) — wrong peer ticket?"),
                        }
                    }
                    _ => tracing::debug!(from = ?frame.from, "ignoring a non-TokenMailboxDeposit frame while waiting on anonymous mailbox responses"),
                }
            }
        }
    }

    println!("done — received {received} mailbox item(s)");
    Ok(())
}

async fn group_create() -> Result<()> {
    let boot = bootstrap().await?;
    drop(boot.rx);

    let conversation = ConversationId::new();
    boot.group_service
        .create_group_mls(conversation, boot.local_account)?;
    println!("created group {conversation} — you are the founding admin");
    println!("share this conversation id with members you add");
    Ok(())
}

async fn group_add_member(
    conversation: &str,
    peer_ticket: &str,
    peer_device_id: &str,
    peer_account_id: &str,
    key_package_b64: &str,
) -> Result<()> {
    let boot = bootstrap().await?;
    // This one has to stay alive long enough to actually send the
    // commit/welcome over the network, unlike the drop(boot.rx)
    // one-shots above — `add_member_mls` awaits `self.endpoint.send`,
    // which needs the endpoint's background tasks (spawned inside
    // `SiarEndpoint::bind`) still running, not `rx` specifically. Kept
    // for symmetry with `listen`'s shape rather than because this
    // command reads from it.
    drop(boot.rx);

    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let peer_device = DeviceId::from_uuid(parse_uuid(peer_device_id)?);
    let peer_account = AccountId::from_uuid(parse_uuid(peer_account_id)?);
    let ticket = PeerTicket::decode(peer_ticket).context("decoding peer ticket")?;
    let key_package_bytes = base64_decode(key_package_b64).context("decoding key package bytes")?;

    boot.device_directory.register(
        peer_account,
        MemberDevice {
            device_id: peer_device,
            ticket,
        },
    );

    boot.group_service
        .add_member_mls(conversation, peer_account, peer_device, &key_package_bytes)
        .await?;
    println!("added {peer_account} to group {conversation} — commit and welcome sent");

    // `join_group_mls` needs this group's current bookkeeping
    // (membership/roles/epoch) as an explicit argument — see that
    // method's own doc comment for why it isn't derived automatically.
    // Relay it the same hand-paired way everything else in this CLI
    // moves between processes.
    let state = boot
        .group_service
        .group_state(conversation)?
        .context("just-updated group has no local state — this is a bug")?;
    let state_bytes = postcard::to_allocvec(&state).context("encoding group state")?;
    println!(
        "group state for the new member's `join-group` (base64): {}",
        base64_encode(&state_bytes)
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn group_send(conversation: &str, text: &str) -> Result<()> {
    let boot = bootstrap().await?;
    drop(boot.rx);

    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let text = MessageText::parse(text.to_string()).context("message text")?;

    let message_id = boot.group_service.send_text_mls(conversation, text).await?;
    println!("sent {message_id} to group {conversation}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Same cross-process limitation this file's top doc comment flags:
/// this only works if `welcome_b64`/`state_b64` correspond to a key
/// package `publish_key_package` generated *in this same process* and
/// never consumed — since `pending_identity` (see that field's doc
/// comment) doesn't survive a process exit, and this command's own
/// `bootstrap()` call always starts a fresh `GroupService`, running
/// this as a separate invocation after `publish-key-package` will
/// always fail with `NoPendingKeyPackageIdentity`. `listen
/// --publish-key-package`'s in-loop prompt is the flow that actually
/// works, because publishing and joining share one process there. This
/// command stays because its logic is correct and useful on its own —
/// e.g. scripting `join_group_mls` directly against welcome bytes
/// obtained some other way — just not as a "run this after
/// publish-key-package" two-step.
async fn join_group(conversation: &str, welcome_b64: &str, state_b64: &str) -> Result<()> {
    let boot = bootstrap().await?;
    drop(boot.rx);

    let conversation = ConversationId::from_uuid(parse_uuid(conversation)?);
    let welcome_bytes = base64_decode(welcome_b64).context("decoding welcome bytes")?;
    let state_bytes = base64_decode(state_b64).context("decoding group state bytes")?;
    let state: siar_domain::GroupState =
        postcard::from_bytes(&state_bytes).context("decoding group state")?;

    boot.group_service
        .join_group_mls(conversation, &welcome_bytes, state)?;
    println!(
        "joined group {conversation} — run `listen` in this process to receive group messages"
    );
    Ok(())
}

fn parse_uuid(s: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(s).with_context(|| format!("'{s}' is not a valid id"))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .context("invalid base64")
}

fn guess_media_type(path: &str) -> siar_domain::MediaType {
    use siar_domain::MediaType::*;
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => ImagePng,
        "jpg" | "jpeg" => ImageJpeg,
        "webp" => ImageWebp,
        "opus" => AudioOpus,
        "mp4" => VideoMp4,
        _ => Other,
    }
}

fn hex_preview(bytes: &[u8; 32]) -> String {
    bytes[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        + "…"
}
