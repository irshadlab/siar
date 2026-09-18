//! next.md §81: a dedicated relay node — Raspberry Pi, old laptop, or
//! small Linux box, running BLE/Wi-Fi/Ethernet/Iroh with a large DTN
//! store. Phase 7 of `next.md`'s roadmap.
//!
//! What this binary does: generates or loads a *persistent* identity
//! (unlike `apps/cli`'s throwaway `DeviceIdentity::generate()` every
//! run — a relay node needs the same identity across restarts so peers
//! can keep trusting it), binds a [`SiarEndpoint`] (reusing every
//! transport already built — Iroh direct/relay, LAN mDNS from Phase 1),
//! prints its connection ticket the same way `apps/cli`'s `listen`
//! does, and constructs the DTN/routing machinery below.
//!
//! **Reconciliation note** (see `MIGRATION.md`'s device-certificate
//! section): this file used to be built against `siar-routing`/
//! `siar-dtn`, both since retired. Routing is now real
//! `siar_connectivity::TransportManager` (backed by
//! `siar_routing_policy`'s `link_health`/`relay_composition`/
//! `congestion` — the three genuine gaps ported from `siar-routing`).
//! DTN storage/dedup is now [`bundle_store`], a small module local to
//! *this binary* rather than a port onto `siar-dtn-bundle`: that
//! crate's `DtnBundle` addresses destinations with an opaque
//! `RouteToken`, which is a real, deliberate privacy design this
//! relay's actual wire format doesn't yet support — `MeshEnvelope`
//! (this workspace's only real wire representation of mesh/DTN
//! traffic) still carries a plain `DeviceId` destination (see that
//! struct's own doc comment: an independent, already-flagged gap).
//! [`bundle_store::StoredBundle`] mirrors `MeshEnvelope` directly
//! instead of pretending an opacity the wire format doesn't provide.
//!
//! - **Routing**: `TransportManager::sync_local_peers` runs on a timer,
//!   keeping its `PathCandidate` table current with whichever peers
//!   Phase 1's mDNS discovery can currently see on the LAN.
//!   `TransportManager::device_for`/`record_device_endpoint` is the
//!   `EndpointId <-> DeviceId` join a `MailboxCheckIn`'s self-
//!   disclosure feeds — the one real signal this relay has for that
//!   mapping — used to push a bundle proactively to its destination's
//!   last-known endpoint the moment it arrives, instead of only
//!   reactively flooding it to whoever happens to make contact next.
//! - **Mailbox authentication**: `siar_protocol::mailbox::
//!   DeviceKeyDirectory` verifies a `MailboxCheckIn`'s Ed25519
//!   signature and freshness window, and pins its claimed device's key
//!   trust-on-first-use, before this relay trusts it for anything —
//!   including the `TransportManager::record_device_endpoint` call
//!   above, which would otherwise be trusting the exact same
//!   unauthenticated claim it acts on.
//!
//! The naive-flood fallback (for a bundle whose destination has no
//! known endpoint hint yet) orders its candidates by
//! `TrafficPriority`, narrowed by `siar_routing_policy::congestion::
//! CongestionTracker::congestion_ceiling` once genuinely backed up —
//! `bundle_store` itself is the real backlog `CongestionTracker`
//! reports on, not a second queue (see that type's own doc comment).
//!
//! What's still NOT real destination-aware routing, flagged rather than
//! oversold: `TransportManager` only ever learns an `EndpointId <->
//! DeviceId` mapping from a device's own voluntary check-in with *this*
//! relay specifically — there's still no way to learn "device X is
//! reachable via peer Y" from ordinary traffic, since `MeshEnvelope`
//! deliberately carries no sender identity (next.md §73-74's mesh-
//! privacy design). A destination that's never checked in with this
//! relay is still only reachable via the naive flood.
//!
//! Real, bounded 2-hop composition exists (`siar_routing_policy::
//! relay_composition::compose_via_relay`, driven by real
//! `WireMessage::RouteAdvertisement`s this relay both sends and
//! receives — see `route_advertisement.rs`'s doc comment for the
//! message shape and its deliberately unauthenticated trust model) but
//! composing now needs *both* the advertiser and the claimed
//! destination resolved to a `DeviceId` first, which this relay can
//! only do for a device that has checked in with it directly — see the
//! `WireMessage::RouteAdvertisement` receive arm below for the real,
//! named narrowing that follows from `PathCandidate` being `DeviceId`-
//! keyed rather than the retired `PathTable`'s `EndpointId` keying.
//! Multi-hop beyond one relay, BLE<->Wi-Fi upgrades, and gateway
//! bridging remain unbuilt. Mailbox authentication is still only half
//! of next.md §32 — see `mailbox.rs`'s own doc comment for the
//! unlinkability half that remains open on purpose.
//!
//! The RTT/reliability half of congestion detection
//! (`siar_routing_policy::link_health::LinkHealth`, wired into
//! `TransportManager::record_send_outcome`) has a real caller: every
//! `endpoint.send` in this file goes through the `send_and_record`
//! helper, which times the attempt and folds the outcome back into
//! `TransportManager` — see that function's own doc comment for the
//! one honest approximation it makes.
//!
//! This relay also runs `siar_protocol::mailbox::TokenMailboxStore` —
//! the token-keyed counterpart to `bundle_store`, filled by
//! `WireMessage::TokenMailboxDeposit` and drained by
//! `WireMessage::AnonymousMailboxCheckIn`. Unlike the `MailboxCheckIn`
//! arm, there's no signature check here — presenting a token *is* the
//! authorization (see `siar_crypto::mailbox_token`'s own doc comment
//! for that bearer-capability tradeoff). `apps/cli` has a real
//! sender/receiver for this path (`send-anon`/`check-mailbox-anon`);
//! `apps/desktop` still only builds/sends the `DeviceId`-addressed
//! `MailboxCheckIn` — wiring the same choice into its UI is separate,
//! real follow-up work.
//!
//! `siar_messaging::MessageService::handle_incoming` requires a
//! `&PeerTicket` — the sender's public keys, known *in advance* — to
//! decrypt anything (see `apps/cli`'s own `listen` mode, which takes
//! exactly one peer ticket up front). `WireMessage::V1` frames from an
//! unpaired stranger still can't be processed by this relay for that
//! reason; a stranger's traffic has to arrive as a `MeshEnvelope`
//! instead, whose whole design point (see `siar-protocol::mesh`'s own
//! doc comment) is not needing a session at all.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use siar_connectivity::TransportManager;
use siar_crypto::DeviceIdentity;
use siar_domain::{DeviceId, MessagePriority};
use siar_protocol::{
    DeviceKeyDirectory, RouteAdvertisement, TokenMailboxEnvelope, TokenMailboxStore, WireMessage,
};
use siar_protocol_ext::lifecycle::TrafficPriority;
use siar_routing_policy::congestion::CongestionTracker;
use siar_routing_policy::link_health::SendOutcome;
use siar_routing_policy::relay_composition::{compose_via_relay, RelayAdvertisement};
use siar_routing_policy::types::TransportKind;
use siar_transport::{PeerTransport, SiarEndpoint};
use tokio::sync::mpsc;

mod bundle_store;
use bundle_store::{BundleStore, SeenIds, StoredBundle};

/// next.md §68: "Emergency: configurable 500 MB – several GB" — this
/// binary's own default, not a hard limit; override with
/// `--quota-bytes`.
const DEFAULT_QUOTA_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
const DEFAULT_SEEN_CAPACITY: usize = 100_000;
const DEFAULT_CONGESTION_CAPACITY_PER_TIER: usize = 1024;
/// Fraction of a throttled tier's capacity that counts as "backed up"
/// for `CongestionTracker::congestion_ceiling` (see that method's doc
/// comment). Chosen conservatively — react to backlog before a queue
/// is anywhere near actually full and starting to reject new items —
/// not tuned against real traffic, same status every other constant in
/// this file carries.
const CONGESTION_OCCUPANCY_THRESHOLD: f32 = 0.5;
/// How long a `TransportManager` candidate or device-endpoint hint is
/// trusted before `remove_stale` drops it — next.md §92's "mobile
/// topology changes too quickly" reasoning, same one
/// `TransportManager::remove_stale`'s own doc comment already gives.
/// Ten minutes, not a value next.md specifies anywhere: a relay node
/// is meant to be relatively stationary (Raspberry Pi / small box, per
/// this file's own top doc comment), so this favors "stale entries get
/// cleaned up eventually" over guessing at a tighter mobile-handset-
/// appropriate number this binary doesn't need.
const ROUTE_STALE_AFTER_MILLIS: u64 = 10 * 60 * 1000;
/// How often the periodic sync/cleanup task runs.
const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// Freshness window for a `MailboxCheckIn`'s signature — see
/// `siar_protocol::mailbox::MailboxCheckIn::verify`'s own doc comment
/// for why this doubles as clock-skew tolerance on both sides. Five
/// minutes: generous enough that ordinary clock drift between a phone
/// and this relay won't cause spurious rejections, tight enough that a
/// captured check-in can't be replayed hours or days later.
const MAILBOX_CHECKIN_MAX_AGE_MILLIS: u64 = 5 * 60 * 1000;

/// This relay's own reasoned mapping onto `siar_protocol_ext`'s
/// 6-tier `TrafficPriority` vocabulary — `siar_domain::MessagePriority`
/// (5 tiers: Emergency/Critical/Interactive/Normal/Background) has no
/// tier as narrow as `TrafficPriority::Control`, so `Emergency` maps
/// onto `Critical` (the most urgent tier available) rather than
/// `Control` (reserved, per that type's own doc comment, for protocol-
/// level control traffic this relay doesn't originate any of).
fn traffic_priority_for(priority: MessagePriority) -> TrafficPriority {
    match priority {
        MessagePriority::Emergency => TrafficPriority::Critical,
        MessagePriority::Critical => TrafficPriority::Control,
        MessagePriority::Interactive | MessagePriority::Normal => TrafficPriority::Normal,
        MessagePriority::Background => TrafficPriority::Background,
    }
}

struct Config {
    identity_path: PathBuf,
    quota_bytes: u64,
    seen_capacity: usize,
}

impl Config {
    fn from_args() -> Self {
        let mut config = Self {
            identity_path: default_identity_path(),
            quota_bytes: DEFAULT_QUOTA_BYTES,
            seen_capacity: DEFAULT_SEEN_CAPACITY,
        };

        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--identity" => {
                    if let Some(value) = args.next() {
                        config.identity_path = PathBuf::from(value);
                    }
                }
                "--quota-bytes" => {
                    if let Some(value) = args.next().and_then(|v| v.parse().ok()) {
                        config.quota_bytes = value;
                    }
                }
                "--seen-capacity" => {
                    if let Some(value) = args.next().and_then(|v| v.parse().ok()) {
                        config.seen_capacity = value;
                    }
                }
                other => {
                    tracing::warn!(flag = other, "unrecognized flag, ignoring");
                }
            }
        }

        config
    }
}

fn default_identity_path() -> PathBuf {
    // next.md §81's target platforms (Raspberry Pi / small Linux box)
    // are exactly where $HOME is reliably set; falling back to the
    // current directory rather than panicking if it somehow isn't —
    // a relay node refusing to start over a missing env var is a worse
    // failure mode than writing its identity file next to the binary.
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".siar-emergency-node").join("identity.bin")
}

fn load_or_create_identity(path: &std::path::Path) -> Result<DeviceIdentity> {
    if path.exists() {
        return DeviceIdentity::load_from_file(path).context("loading existing identity");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating identity directory")?;
    }
    let identity = DeviceIdentity::generate();
    identity.save_to_file(path).context("saving new identity")?;
    tracing::info!(path = %path.display(), "generated new relay node identity");
    Ok(identity)
}

/// A stored bundle carries everything a `MeshEnvelope` needs (see
/// `bundle_store::StoredBundle`'s own doc comment) — shared by both
/// this file's forward-on-contact step and its mailbox check-in
/// handler, which both need to turn a stored bundle back into wire
/// bytes.
fn bundle_to_envelope(bundle: StoredBundle) -> siar_protocol::MeshEnvelope {
    siar_protocol::MeshEnvelope {
        id: bundle.id,
        destination: bundle.destination,
        created_at: bundle.created_at,
        expires_at: bundle.expires_at,
        hop_limit: bundle.hop_limit,
        priority: bundle.priority,
        payload_hash: bundle.payload_hash,
        ciphertext: bundle.ciphertext,
    }
}

/// Times a real `SiarEndpoint::send` attempt and folds the outcome into
/// `TransportManager::record_send_outcome`, which this workspace's
/// `siar_routing_policy::link_health::LinkHealth` port closed as a
/// real gap. Every real outbound send in this file now goes through
/// this instead of calling `endpoint.send` directly, so a candidate's
/// `rtt_millis`/`packet_loss` fields stop being permanent
/// `None`/absent placeholders the moment this relay actually talks to
/// anyone.
///
/// `known_addr`, when the caller happens to already have the
/// destination's full `iroh::EndpointAddr` in hand (not just its
/// `EndpointId`), is classified via `siar_connectivity::
/// candidate_source::classify_endpoint_addr` — real evidence-based
/// `LocalLan`/`IrohDirect`/`IrohRelay` distinction instead of the
/// blanket `IrohDirect` default this function used everywhere before.
/// `None` (most call sites in this file only ever have a bare
/// `EndpointId` from an incoming frame's sender, not its full
/// addressing) falls back to that same `IrohDirect` default — see
/// `classify_endpoint_addr`'s own doc comment for exactly what the
/// classification is and isn't (advertised reachability, not a
/// measured path either way).
async fn send_and_record(
    endpoint: &SiarEndpoint,
    transport_manager: &TransportManager,
    destination: iroh::EndpointId,
    known_addr: Option<&iroh::EndpointAddr>,
    message: &WireMessage,
) -> Result<(), siar_transport::TransportError> {
    let started = std::time::Instant::now();
    let result = endpoint
        .send(iroh::EndpointAddr::new(destination), message)
        .await;
    let elapsed_millis = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    let outcome = match &result {
        Ok(()) => SendOutcome::success(elapsed_millis),
        Err(_) => SendOutcome::failure(),
    };
    let kind = known_addr
        .map(siar_connectivity::candidate_source::classify_endpoint_addr)
        .unwrap_or(TransportKind::IrohDirect);
    transport_manager.record_send_outcome(destination, kind, outcome);
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let config = Config::from_args();

    let identity = load_or_create_identity(&config.identity_path)?;
    let device_id = DeviceId::new();

    // Same bootstrap shape as `apps/cli`'s `bootstrap()` — in-memory
    // storage for now, same as that CLI; a relay node's own message/
    // outbox/blob persistence (as opposed to the DTN bundle store below,
    // which is what actually needs to survive a restart for this
    // binary's purpose) is real work this phase didn't need to touch.
    let db = siar_storage::open_in_memory().context("opening local database")?;
    let messages = Arc::new(siar_storage::StoolapMessageRepository::new(db.clone()));
    let outbox = Arc::new(siar_storage::StoolapOutboxRepository::new(db.clone()));
    let blobs: Arc<dyn siar_storage::BlobRepository + Send + Sync> =
        Arc::new(siar_storage::StoolapBlobRepository::new(db));
    let blob_store: Arc<dyn siar_transport::BlobStore> =
        Arc::new(siar_messaging::StorageBlobStore(blobs.clone()));

    let (tx, mut rx) = mpsc::channel::<siar_transport::IncomingFrame>(256);
    let iroh_secret = iroh::SecretKey::generate();
    let endpoint = Arc::new(
        SiarEndpoint::bind(iroh_secret, tx, blob_store)
            .await
            .context("binding endpoint")?,
    );

    let ticket = siar_messaging::PeerTicket {
        endpoint_addr: endpoint.addr(),
        x25519_public: identity.x25519_public().to_bytes(),
        ed25519_verifying: identity.verifying_key().to_bytes(),
    };
    // next.md §111: a relay node's own contact card, the same concept
    // as a phone's QR pairing — just printed as text on a terminal
    // instead of rendered as a QR image.
    tracing::info!(ticket = %ticket.encode(), "emergency relay node ready");

    let _service = Arc::new(siar_messaging::MessageService::new(
        device_id,
        identity,
        endpoint.clone(),
        messages,
        outbox,
        blobs,
    ));

    // next.md §68's DTN storage, §31's dedup, §91's path table (now
    // `TransportManager`), §93's scheduler (now `CongestionTracker`) —
    // see this file's top doc comment for how they're actually wired
    // together now. `Mutex`, not the async-aware channel types
    // elsewhere in this workspace: everything below is short
    // synchronous critical sections (lock, read/mutate, drop before
    // any `.await` — see the receive loop's own comments on why that
    // ordering matters), never held across a suspend point.
    let bundle_store: Mutex<BundleStore> = Mutex::new(BundleStore::new(config.quota_bytes));
    let seen: Mutex<SeenIds<siar_domain::MessageId>> =
        Mutex::new(SeenIds::new(config.seen_capacity));
    let transport_manager = Arc::new(TransportManager::new(endpoint.clone()));
    let device_keys: Mutex<DeviceKeyDirectory> = Mutex::new(DeviceKeyDirectory::new());
    // The unlinkable counterpart to `bundle_store` — see
    // `siar_protocol::mailbox::TokenMailboxStore`'s own doc comment for
    // why this needs to be a structurally separate store rather than a
    // second index into `bundle_store`. Reuses the same `seen`
    // dedup set as `bundle_store`'s `Mesh` path below (a `MessageId` is
    // globally unique regardless of which addressing scheme a given
    // message used, so one dedup set correctly covers both).
    let token_mailbox: Mutex<TokenMailboxStore<TokenMailboxEnvelope>> =
        Mutex::new(TokenMailboxStore::new());

    // Keeps `TransportManager`'s candidates (and the `DeviceRoutes`
    // hints backing them) current on a timer — next.md §92's "mobile
    // topology changes too quickly" applies to both: a LAN peer that's
    // walked out of mDNS range, or a device whose `MailboxCheckIn`
    // endpoint hint is stale, should both stop being trusted eventually
    // rather than lingering forever.
    //
    // The same tick also *sends* `RouteAdvertisement`s — the other half
    // of the exchange `WireMessage::RouteAdvertisement`'s receive-side
    // arm below consumes. Deliberately narrow, matching
    // `route_advertisement.rs`'s own "no propagation policy beyond
    // this" doc comment: advertises only this relay's own *direct*
    // routes, to every currently-known local peer, once per tick — no
    // fan-out beyond that. Every candidate `TransportManager` currently
    // holds genuinely is direct — nothing in this workspace populates
    // it via `relay_composition::compose_via_relay` yet (see this
    // file's own `WireMessage::RouteAdvertisement` handling below,
    // which only *consumes* that composition, not produces it) — so
    // there's no `NextHop`-style filter to apply here the way the
    // retired `PathTable` needed one; that distinction would only
    // start mattering once something starts composing.
    {
        let transport_manager = transport_manager.clone();
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SYNC_INTERVAL);
            loop {
                interval.tick().await;
                let now = siar_domain::now_millis();
                transport_manager.sync_local_peers(now);
                transport_manager.remove_stale(now, ROUTE_STALE_AFTER_MILLIS);

                // Snapshot of (destination endpoint bytes, candidate)
                // pairs, resolved and the lock dropped before any
                // `.await` below — same "resolve then act" split this
                // file's other `Mutex`-guarded sections already use for
                // the same reason (holding a `MutexGuard` across
                // `endpoint.send(...).await` would block every other
                // task waiting on this same lock for the duration of a
                // network send). `candidate.endpoint.0` is literally
                // the destination's own `EndpointId` bytes for every
                // candidate `sync_local_peers` constructs (see that
                // method's own body) — no separate lookup needed to
                // recover it.
                let direct_routes: Vec<([u8; 32], siar_routing_policy::candidate::PathCandidate)> = {
                    transport_manager
                        .candidates()
                        .values()
                        .filter_map(|candidate| {
                            let bytes: [u8; 32] =
                                candidate.endpoint.0.as_slice().try_into().ok()?;
                            Some((bytes, candidate.clone()))
                        })
                        .collect()
                };
                if direct_routes.is_empty() {
                    continue;
                }

                let peers = endpoint.local_peers();
                for peer in &peers {
                    for (destination_endpoint, candidate) in &direct_routes {
                        // Advertising a peer's own route back to itself
                        // is a pure no-op for the receiver
                        // (`compose_via_relay` would need a route *to*
                        // the peer, not *from* it) and would be the
                        // simplest possible loop.
                        if *destination_endpoint == *peer.id.as_bytes() {
                            continue;
                        }
                        let reliability = candidate
                            .metrics
                            .packet_loss
                            .map(|loss| (1.0 - loss.get()) as f32)
                            .unwrap_or(1.0);
                        let advertisement = WireMessage::RouteAdvertisement(RouteAdvertisement {
                            destination_endpoint: *destination_endpoint,
                            rtt_millis: candidate.metrics.rtt_millis,
                            reliability,
                            advertised_at: now,
                        });
                        if let Err(e) = send_and_record(
                            &endpoint,
                            &transport_manager,
                            peer.id,
                            Some(peer),
                            &advertisement,
                        )
                        .await
                        {
                            tracing::debug!(error = %e, peer = ?peer.id, "route advertisement send failed");
                        }
                    }
                }
            }
        });
    }

    tracing::info!(
        quota_bytes = config.quota_bytes,
        seen_capacity = config.seen_capacity,
        "DTN store/dedup ready — destination-aware push via TransportManager for devices that have checked in, priority-ordered flood fallback otherwise"
    );

    while let Some(frame) = rx.recv().await {
        // Set inside the `Mesh` arm below when this iteration stored a
        // bundle AND already pushed it directly to a known destination
        // endpoint (see `DeviceRoutes` below) — skipped from the
        // fallback flood step so it isn't sent twice and doesn't
        // double-consume its `replication_budget`.
        let mut already_pushed: Option<siar_domain::MessageId> = None;
        // Set when this iteration's frame came from a peer we just
        // received a *new* bundle from — kept from the fallback flood
        // for the same "don't hand it straight back to where it came
        // from" reason the naive version of this loop already had.
        let mut just_received: Option<siar_domain::MessageId> = None;

        match frame.message {
            WireMessage::V1(_envelope) => {
                // `MessageService::handle_incoming` needs a `&PeerTicket`
                // known in advance to decrypt anything — this node can't
                // do that for an arbitrary, not-pre-paired sender, which
                // is exactly the gap this file's top doc comment
                // describes. A stranger's traffic needs to arrive as a
                // `MeshEnvelope` instead (see below) — this branch is
                // for the "someone sent this relay an ordinary paired
                // message" case, which just isn't useful for a relay
                // with no conversation partner of its own yet.
                tracing::info!(from = ?frame.from, "received a V1 frame from a peer with no pre-shared ticket — dropping");
            }
            WireMessage::MailboxCheckIn(check_in) => {
                // Verified and pinned before this check-in is trusted
                // for anything — see `siar_protocol::mailbox`'s doc
                // comment for exactly what a passing `verify_and_pin`
                // does and doesn't prove. A rejected check-in gets
                // neither an answer nor a `DeviceRoutes` recording:
                // treating it as silently absent is the correct
                // response to an unverifiable identity claim, not an
                // error worth this relay's own logging budget beyond a
                // debug line.
                let verified = device_keys
                    .lock()
                    .expect("DeviceKeyDirectory lock poisoned")
                    .verify_and_pin(
                        &check_in,
                        siar_domain::now_millis(),
                        MAILBOX_CHECKIN_MAX_AGE_MILLIS,
                    );
                if let Err(e) = verified {
                    tracing::debug!(error = %e, device = ?check_in.device, from = ?frame.from, "rejected an unverifiable mailbox check-in");
                    // `continue` skips this iteration's flood-fallback
                    // step too, not just the check-in response — a
                    // deliberate, slightly stricter choice than the
                    // "flood to any contact regardless of frame
                    // validity" reasoning the flood step's own comment
                    // gives for a `V1` frame: an invalid signature is a
                    // stronger signal of a hostile or broken peer than
                    // "we simply can't decrypt this," so this relay
                    // spends no forwarding effort on that contact
                    // either.
                    continue;
                }

                // The self-disclosure moment `TransportManager`'s
                // `DeviceRoutes` join exists for (see that module's own
                // doc comment) — recorded before answering, so a
                // bundle for this exact device that arrives later in
                // this same process's lifetime can be pushed to it
                // directly instead of waiting for another check-in.
                // Only reached once `verify_and_pin` above has actually
                // confirmed this device controls the key it claims —
                // this no longer trusts a bare, unauthenticated
                // assertion the way it would have before that pass.
                transport_manager.record_device_endpoint(
                    check_in.device,
                    frame.from,
                    siar_domain::now_millis(),
                );

                // next.md §76–77's mailbox check-in — see
                // `siar-protocol::mailbox`'s doc comment for what this
                // does and doesn't authenticate. Unlike the naive
                // forward-on-contact step below, this is a *destination*
                // explicitly asking, so a match here is a real, direct
                // delivery — `mark_delivered` afterward, not gated by
                // `consume_for_forward`'s replication-budget check
                // (`MeshBundle::try_consume_replication`'s own doc
                // comment already draws this same distinction: direct
                // delivery to a known destination is separate from the
                // budget that gates opportunistic replication).
                let matching_ids: Vec<siar_domain::MessageId> = bundle_store
                    .lock()
                    .expect("BundleStore lock poisoned")
                    .iter()
                    .filter(|bundle| bundle.destination == check_in.device)
                    .map(|bundle| bundle.id)
                    .collect();

                let mut delivered_count = 0usize;
                for id in matching_ids {
                    let Some(bundle) = bundle_store
                        .lock()
                        .expect("BundleStore lock poisoned")
                        .get(id)
                    else {
                        continue; // evicted between the scan above and now
                    };
                    let envelope = bundle_to_envelope(bundle);
                    match send_and_record(
                        &endpoint,
                        &transport_manager,
                        frame.from,
                        None,
                        &WireMessage::Mesh(envelope),
                    )
                    .await
                    {
                        Ok(()) => {
                            bundle_store
                                .lock()
                                .expect("BundleStore lock poisoned")
                                .mark_delivered(id);
                            delivered_count += 1;
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, id = ?id, "mailbox delivery attempt failed")
                        }
                    }
                }
                tracing::info!(
                    device = ?check_in.device,
                    from = ?frame.from,
                    delivered_count,
                    "answered a mailbox check-in"
                );
            }
            WireMessage::TokenMailboxDeposit(envelope) => {
                // The unlinkable counterpart to the `Mesh` arm below —
                // same dedup/expiry shape, filed into `token_mailbox`
                // (keyed by the sender's chosen `MailboxToken`) instead
                // of `bundle_store` (keyed by `DeviceId`). See
                // `siar_protocol::mailbox`'s doc comments for why these
                // stay two structurally separate stores.
                let now = siar_domain::now_millis();
                let already_seen = seen
                    .lock()
                    .expect("SeenBundles lock poisoned")
                    .check_and_record(envelope.id);
                if already_seen {
                    tracing::debug!(id = ?envelope.id, from = ?frame.from, "duplicate TokenMailboxEnvelope, dropping");
                    continue;
                }
                if envelope.is_expired(now) {
                    tracing::debug!(id = ?envelope.id, from = ?frame.from, "expired TokenMailboxEnvelope, dropping");
                    continue;
                }
                let token = envelope.destination_token;
                token_mailbox
                    .lock()
                    .expect("TokenMailboxStore lock poisoned")
                    .deposit(token, envelope);
                tracing::info!(from = ?frame.from, "stored a TokenMailboxEnvelope for later collection");
            }
            WireMessage::AnonymousMailboxCheckIn(check_in) => {
                // Bearer-capability model (`siar_crypto::mailbox_token`'s
                // own doc comment): presenting the token *is* the
                // authorization — no signature to verify here, unlike
                // the `MailboxCheckIn` arm above. This relay hands back
                // whatever is filed under it, no questions asked, the
                // same way any bearer-token API would.
                let deposits = token_mailbox
                    .lock()
                    .expect("TokenMailboxStore lock poisoned")
                    .collect(check_in.token);

                let mut delivered_count = 0usize;
                // Failed sends are re-deposited rather than dropped —
                // `collect` above already removed them from the store,
                // and a delivery attempt that fails shouldn't cost the
                // sender their message the way it would if this arm
                // just let a failed `envelope` fall out of scope.
                let mut redeposit = Vec::new();
                for envelope in deposits {
                    let message = WireMessage::TokenMailboxDeposit(envelope.clone());
                    match send_and_record(&endpoint, &transport_manager, frame.from, None, &message)
                        .await
                    {
                        Ok(()) => delivered_count += 1,
                        Err(e) => {
                            tracing::debug!(error = %e, "anonymous mailbox delivery attempt failed, re-depositing");
                            redeposit.push(envelope);
                        }
                    }
                }
                if !redeposit.is_empty() {
                    let mut store = token_mailbox
                        .lock()
                        .expect("TokenMailboxStore lock poisoned");
                    for envelope in redeposit {
                        store.deposit(check_in.token, envelope);
                    }
                }
                tracing::info!(from = ?frame.from, delivered_count, "answered an anonymous mailbox check-in");
            }
            WireMessage::RouteAdvertisement(advertisement) => {
                // The real routing-advertisement exchange
                // `siar_routing::path::PathTable::compose_via_relay`'s
                // own doc comment flagged as not existing anywhere in
                // this workspace — see `route_advertisement.rs`'s doc
                // comment for what this does and, importantly, does NOT
                // verify (no signature; trusting a direct transport
                // peer the same amount the existing naive-flood forward
                // already does, not a new or stronger trust boundary).
                //
                // Real, named narrowing versus the retired
                // `EndpointId`-native `PathTable`: composing now needs
                // *both* the advertiser and the claimed destination
                // resolved to a `DeviceId` first (`PathCandidate` is
                // `DeviceId`-keyed by design — see `siar_connectivity::
                // TransportManager`'s own doc comment), and this relay
                // can only do that resolution for a device that has
                // itself checked in with *this* relay directly. An
                // advertisement about a destination that has only ever
                // checked in with the advertiser, never with us, can't
                // be composed — dropped below, not guessed at.
                let Some(via_device) = transport_manager.device_for(frame.from) else {
                    tracing::debug!(from = ?frame.from, "no known device for the advertiser itself yet, dropping advertisement");
                    continue;
                };
                let Ok(destination_endpoint) =
                    iroh::EndpointId::from_bytes(&advertisement.destination_endpoint)
                else {
                    tracing::debug!(from = ?frame.from, "route advertisement had a malformed destination endpoint, dropping");
                    continue;
                };
                let Some(destination_device) = transport_manager.device_for(destination_endpoint)
                else {
                    tracing::debug!(from = ?frame.from, "no known device for the advertised destination yet, dropping advertisement");
                    continue;
                };
                let relay_advertisement = RelayAdvertisement {
                    via: via_device,
                    destination: destination_device,
                    relay_endpoint: siar_routing_policy::candidate::TransportEndpoint(
                        advertisement.destination_endpoint.to_vec(),
                    ),
                    rtt_millis: advertisement.rtt_millis,
                    reliability: siar_routing_policy::metrics::Ratio::new(
                        advertisement.reliability as f64,
                    ),
                    last_seen_millis: advertisement.advertised_at,
                };
                let direct_candidates = transport_manager.candidates_for(via_device);
                match compose_via_relay(&direct_candidates, &relay_advertisement) {
                    Some(composed) => {
                        transport_manager
                            .candidates()
                            .insert((composed.peer, composed.transport), composed);
                        tracing::debug!(from = ?frame.from, destination = ?destination_device, "composed and stored a 2-hop route from an advertisement");
                    }
                    None => {
                        // We have no *direct* candidate to `via_device`
                        // ourselves (the precondition `compose_via_relay`
                        // requires) — nothing to compose yet. Not an
                        // error: `TransportManager::sync_local_peers`'s
                        // own periodic tick will supply that direct
                        // candidate once/if it exists, and a later
                        // advertisement will compose successfully then.
                        tracing::debug!(from = ?frame.from, destination = ?destination_device, "no direct candidate to the advertiser yet, dropping advertisement");
                    }
                }
            }
            WireMessage::Mesh(mesh_envelope) => {
                let now = siar_domain::now_millis();

                // next.md §31 dedup: a bundle that's already been seen
                // (forwarded here before, or looped back around) is
                // dropped without touching the store — `check_and_record`
                // both checks and marks in one call.
                let already_seen = seen
                    .lock()
                    .expect("SeenIds lock poisoned")
                    .check_and_record(mesh_envelope.id);
                if already_seen {
                    tracing::debug!(id = ?mesh_envelope.id, from = ?frame.from, "duplicate MeshEnvelope, dropping");
                    continue;
                }

                if mesh_envelope.is_expired(now) {
                    tracing::debug!(id = ?mesh_envelope.id, from = ?frame.from, "expired MeshEnvelope, dropping");
                    continue;
                }

                // `MeshEnvelope`'s own doc comment on `replication_budget`:
                // the wire format doesn't carry one (a relay's own
                // outgoing-copy policy, not the original sender's, per
                // next.md §38's "ordinary DM = 2 copies... SOS = 8
                // copies" framing) — `MessagePriority::
                // default_replication_budget` is exactly that policy,
                // already built in `siar-domain` for this.
                let destination = mesh_envelope.destination;
                let bundle = StoredBundle {
                    id: mesh_envelope.id,
                    destination,
                    payload_hash: mesh_envelope.payload_hash,
                    ciphertext: mesh_envelope.ciphertext,
                    priority: mesh_envelope.priority,
                    hop_limit: mesh_envelope.hop_limit,
                    replication_budget: mesh_envelope.priority.default_replication_budget(),
                    created_at: mesh_envelope.created_at,
                    expires_at: mesh_envelope.expires_at,
                };
                just_received = Some(bundle.id);

                let evicted = bundle_store
                    .lock()
                    .expect("BundleStore lock poisoned")
                    .insert(bundle);
                if evicted.is_empty() {
                    tracing::info!(id = ?mesh_envelope.id, from = ?frame.from, destination = ?destination, "stored MeshEnvelope for later carriage");
                } else {
                    tracing::info!(
                        id = ?mesh_envelope.id,
                        from = ?frame.from,
                        destination = ?destination,
                        evicted_count = evicted.len(),
                        "stored MeshEnvelope, evicting lower-priority bundles to make room"
                    );
                }

                // The real destination-aware improvement this pass adds:
                // if this device has checked in with us before, we
                // already know its endpoint — push the freshly-stored
                // bundle to it right now rather than waiting for it to
                // either check in again or happen to be the next peer
                // this relay hears from.
                let known_endpoint = transport_manager.known_endpoint_for(destination);
                if let Some(target_endpoint) = known_endpoint {
                    if target_endpoint != frame.from {
                        // Resolved to a plain owned `Option<MeshBundle>`
                        // in its own statement first, not matched on
                        // directly — under this workspace's edition
                        // 2021, an `if let`'s scrutinee temporaries (the
                        // `MutexGuard` `.lock()` would produce) live for
                        // the whole `if let` block, which would hold
                        // this lock across the `endpoint.send(...).await`
                        // below. Matching on the already-resolved,
                        // lock-free `consumed` avoids that; the flood
                        // step later in this loop already relies on the
                        // same "resolve then match" split via `let-else`
                        // for the same reason.
                        let consumed = bundle_store
                            .lock()
                            .expect("BundleStore lock poisoned")
                            .consume_for_forward(mesh_envelope.id);
                        if let Some(bundle) = consumed {
                            let envelope = bundle_to_envelope(bundle);
                            let message = WireMessage::Mesh(envelope);
                            match send_and_record(
                                &endpoint,
                                &transport_manager,
                                target_endpoint,
                                None,
                                &message,
                            )
                            .await
                            {
                                Ok(()) => {
                                    already_pushed = Some(mesh_envelope.id);
                                    tracing::info!(
                                        id = ?mesh_envelope.id,
                                        destination = ?destination,
                                        to = ?target_endpoint,
                                        "pushed directly to destination's known endpoint (DeviceRoutes hit)"
                                    );
                                }
                                Err(e) => tracing::debug!(
                                    error = %e,
                                    id = ?mesh_envelope.id,
                                    to = ?target_endpoint,
                                    "destination-aware push failed — falling back to the flood step"
                                ),
                            }
                        }
                        // `consumed` being `None` (hop_limit/
                        // replication_budget already exhausted) just
                        // means nothing to push — falls through to the
                        // flood step below like any other bundle would.
                    }
                }
            }
        }

        // next.md §35's "peer encounter protocol." Any frame from a
        // peer — including a `V1` one this relay couldn't decrypt —
        // means that peer is reachable right now, so it's offered every
        // currently-forwardable stored bundle that wasn't already
        // handled above. This is still a flood for any bundle whose
        // destination `TransportManager` doesn't know an endpoint for
        // yet (next.md §39's full route-scoring needs a live multi-hop
        // path view this binary doesn't have — see this file's top doc
        // comment) — but it's no longer an *unordered* one: candidates
        // are ordered by `TrafficPriority` and, once actually backed
        // up, narrowed by `CongestionTracker::congestion_ceiling` so
        // Emergency-tier bundles are offered before Background-tier
        // ones whenever both are waiting.
        //
        // `bundle_store` itself — not a separate persisted queue — is
        // this loop's real backlog: it's rebuilt fresh from that
        // ground truth every iteration, so a freshly-constructed
        // `CongestionTracker` reporting each current bundle's tier is
        // enough; there's no separate running counter to keep in sync
        // across iterations (see `CongestionTracker`'s own doc comment
        // on why it's occupancy-only, not a second queue — this loop's
        // `bundle_store` is exactly the real storage that doc comment
        // says belongs elsewhere).
        let now = siar_domain::now_millis();
        let candidates: Vec<StoredBundle> = bundle_store
            .lock()
            .expect("BundleStore lock poisoned")
            .iter()
            .filter(|bundle| {
                Some(bundle.id) != just_received
                    && Some(bundle.id) != already_pushed
                    && !bundle.is_expired(now)
            })
            .cloned()
            .collect();

        let mut tracker = CongestionTracker::new(DEFAULT_CONGESTION_CAPACITY_PER_TIER);
        for bundle in &candidates {
            tracker.record_enqueued(traffic_priority_for(bundle.priority));
        }
        // A full tier at this capacity just means this iteration's
        // occupancy report saturates rather than over-counts — not a
        // reason to fail the whole receive loop over a bounded-report
        // ceiling doing exactly what next.md §94 asks of it.
        let ceiling = tracker.congestion_ceiling(CONGESTION_OCCUPANCY_THRESHOLD);

        let mut ordered_bundles: Vec<StoredBundle> = candidates
            .into_iter()
            .filter(|bundle| {
                ceiling
                    .map(|c| traffic_priority_for(bundle.priority) <= c)
                    .unwrap_or(true)
            })
            .collect();
        // `TrafficPriority`'s derived `Ord` follows its own declared
        // tier order (`Critical` first, `Background` last) — an
        // ascending sort is exactly "most urgent offered first."
        ordered_bundles.sort_by_key(|bundle| traffic_priority_for(bundle.priority));
        let ordered_ids: Vec<siar_domain::MessageId> = ordered_bundles
            .into_iter()
            .map(|bundle| bundle.id)
            .collect();

        for id in ordered_ids {
            let Some(bundle) = bundle_store
                .lock()
                .expect("BundleStore lock poisoned")
                .consume_for_forward(id)
            else {
                continue; // hop_limit or replication_budget already exhausted
            };
            let envelope = bundle_to_envelope(bundle);
            let message = WireMessage::Mesh(envelope);
            if let Err(e) =
                send_and_record(&endpoint, &transport_manager, frame.from, None, &message).await
            {
                tracing::debug!(error = %e, id = ?id, to = ?frame.from, "forward attempt failed");
            } else {
                tracing::debug!(id = ?id, to = ?frame.from, "forwarded a stored bundle on contact");
            }
        }
    }

    Ok(())
}
