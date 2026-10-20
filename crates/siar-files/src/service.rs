//! `FilesService`: the host-facing driver. It owns the engine, the
//! `files/1` receiver, and the table of transfers this device is
//! sending, and turns inbound frames into the right protocol calls.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::file_offer::{FileOffer, FileOfferDecision};
use siar_blob_manifest::gc::GcReport;
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, TransferId};
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::{BlobRole, ObjectId, RefOp, ReferenceStore};
use siar_blob_manifest::scheduling::{
    AppNamespace, FairScheduler, PathKind, SchedulerConfig, SchedulerEntry, TransferClass,
};
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::wire::{
    decode_control, encode_control, peek_chunk_transfer_id, peek_manifest_transfer_id, CancelV1,
    ChunkRequestV1, FileControlV1, FileOfferV1, FilesCapabilities, NegotiatedFiles, WireLimits,
};
use siar_domain::{DeviceId, MediaType};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::EventOrigin;
use siar_file_transfer_service::gc_schedule::GcScheduler;
use siar_file_transfer_service::protocol::{
    ChunkOutcome, FileReceiver, FileSender, ReceiveContext, SendStatus,
};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use tokio::sync::{mpsc, oneshot};

use crate::manifest_cache::Lru;
use crate::offer_meta::{self, Decoded, OfferMetadata};
use crate::ReceivedFile;
use crate::{AccessLedger, MemoryAccessLedger};
use crate::{
    BoxFut, Destination, FileChannel, FileSource, FilesError, FrameKind, PairwiseSecrets, ServeGate,
};
use siar_blob_manifest::app_policy::{
    AcceptancePrompt, AutoAction, AutoDownloadPolicy, NetworkKind, NotificationSink,
    TransferNotification,
};
use siar_blob_manifest::fs_store::FsBlobReader;
use siar_blob_manifest::ids::BlobEncryptionKey;
use siar_blob_manifest::key_wrapping::{
    unwrap_blob_key, wrap_blob_key, Revocation, RevocationEffect,
};
use siar_blob_manifest::parallelism::{
    effective_parallelism, AdaptiveConcurrency, ResourceContext, Sample,
};
use siar_blob_manifest::plaintext_cache::{PlaintextCache, PlaintextPolicy};

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// §205 `FileSendPolicy::default()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSendPolicy {
    pub media_type: MediaType,
    pub class: TransferClass,
    /// Offer expiry (§154), milliseconds since the epoch.
    pub expiry_millis: Option<u64>,
    /// A display name the receiver sees (untrusted on their side; at most
    /// 255 bytes). Travels with the key inside the offer — see
    /// `offer_meta` for why that is as private as the channel.
    pub file_name: Option<String>,
    /// How the receiver gets the blob key. See [`KeyDelivery`].
    pub key_delivery: KeyDelivery,
}

/// How the blob key reaches the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyDelivery {
    /// The raw key travels inside the offer: private exactly as far as the
    /// authenticated end-to-end connection carrying it. The simple
    /// default for a direct device-to-device send.
    #[default]
    InOffer,
    /// Only a copy wrapped for this one recipient travels (needs
    /// `FilesConfig::pairwise_secrets`). Required for anything that
    /// crosses a path that is not end-to-end private, and what
    /// [`FilesService::send_file_to_many`] is for.
    Wrapped,
}

/// What an incoming offer's metadata turned out to give this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    /// The key is available: the file will be openable once it arrives.
    Present,
    /// A wrapped key arrived but could not be opened (no pairwise secret
    /// for the sender, wrong recipient, or tampering). The bytes can still
    /// be received and verified; they cannot be opened.
    WrapUnopenable,
    /// The offer carried no usable metadata.
    Absent,
}

impl Default for FileSendPolicy {
    fn default() -> Self {
        Self {
            media_type: MediaType::Other,
            class: TransferClass::UserRequested,
            expiry_millis: None,
            file_name: None,
            key_delivery: KeyDelivery::InOffer,
        }
    }
}

/// §202 `accept(destination_policy)`: who will own the received blob.
#[derive(Debug, Clone)]
pub struct DestinationPolicy {
    pub object: ObjectId,
    pub role: BlobRole,
    pub expires: TransferExpiry,
}

/// §145: offers the policy approves are accepted without asking the
/// application, into the destination this function picks.
pub struct AutoDownload {
    pub policy: AutoDownloadPolicy,
    pub destination: Arc<dyn Fn(&FileOffer) -> DestinationPolicy + Send + Sync>,
}

pub struct FilesConfig {
    pub namespace: NamespaceId,
    pub origin: EventOrigin,
    pub limits: WireLimits,
    pub capabilities: FilesCapabilities,
    /// Offers waiting for the application; a full queue answers `Defer`.
    pub offer_queue: usize,
    /// §183: which product module this service's transfers belong to.
    pub app: AppNamespace,
    /// §181-183: how chunk sending is shared between transfers.
    pub scheduler: SchedulerConfig,
    /// §63-64: how often a GC sweep is due, and the grace period.
    pub gc_interval_millis: u64,
    pub gc_grace_millis: u64,
    /// §127: off (`OnDemand`) unless the host opts in.
    pub plaintext: PlaintextPolicy,
    /// §143: where completed / failed / action-required events go.
    pub notifier: Option<Arc<dyn NotificationSink>>,
    /// §145: automatic acceptance, off unless configured.
    pub auto_download: Option<AutoDownload>,
    /// §43: adapt concurrency per peer from reported [`Sample`]s, inside
    /// the deterministic limits. Off by default ("start with
    /// deterministic limits").
    pub adaptive_concurrency: bool,
    /// A receiving transfer with no progress for this long is re-requested
    /// by [`FilesService::retry_stalled`].
    pub stall_millis: u64,
    /// Who may pull which blob. `None`: any referenced blob (see
    /// [`ServeGate`]).
    pub serve_gate: Option<Arc<dyn ServeGate>>,
    /// How long [`FilesService::fetch_blob`] waits for a blob to arrive.
    pub fetch_timeout_millis: u64,
    /// How long it waits for the peer to react *at all* (an offer or a
    /// refusal). A peer that never answers — it does not speak `files/1`,
    /// or has not paired this device for it — would otherwise cost the
    /// whole `fetch_timeout_millis` before any fallback could run.
    pub fetch_first_response_millis: u64,
    /// Source of per-peer secrets for wrapping and unwrapping blob keys.
    pub pairwise_secrets: Option<Arc<dyn PairwiseSecrets>>,
    /// Who holds which keys and whose access is revoked. `None` uses an
    /// in-memory ledger that forgets on restart (see `access_ledger`).
    pub access_ledger: Option<Arc<dyn AccessLedger>>,
}

impl FilesConfig {
    pub fn new(namespace: NamespaceId, origin: EventOrigin) -> Self {
        Self {
            namespace,
            origin,
            limits: WireLimits::default(),
            capabilities: FilesCapabilities::desktop_default(),
            offer_queue: 32,
            app: AppNamespace(0),
            scheduler: SchedulerConfig::default(),
            gc_interval_millis: 10 * 60 * 1000,
            gc_grace_millis: 24 * 60 * 60 * 1000,
            plaintext: PlaintextPolicy::default(),
            notifier: None,
            auto_download: None,
            adaptive_concurrency: false,
            stall_millis: 15_000,
            serve_gate: None,
            fetch_timeout_millis: 60_000,
            fetch_first_response_millis: 8_000,
            pairwise_secrets: None,
            access_ledger: None,
        }
    }
}

struct Outgoing {
    to: DeviceId,
    blob_id: BlobId,
    manifest: Arc<siar_blob_manifest::manifest::BlobManifest>,
    object: ObjectId,
    class: TransferClass,
    paused: bool,
    /// Chunk indices the receiver asked for and we have not sent yet.
    queue: VecDeque<u32>,
    sender: Option<Arc<FileSender<FsBlobReader>>>,
    /// The offer, as sent, so an unanswered one can be sent again.
    offer_frame: Vec<u8>,
    offered_at_millis: u64,
    /// The receiver has replied to the offer in some way.
    answered: bool,
    resends: u8,
}

/// Everything about the blob being offered (as opposed to who it goes to
/// and how): bundled so the send paths stay readable.
struct BlobToSend {
    descriptor: siar_blob_manifest::descriptor::BlobDescriptor,
    blob_id: BlobId,
    manifest: Arc<siar_blob_manifest::manifest::BlobManifest>,
    /// What travels in the offer's metadata field (the key and name, or
    /// nothing for a pull, where the requester already holds the key).
    metadata: Vec<u8>,
}

/// Times an unanswered offer is re-sent before the transfer is given up.
const MAX_OFFER_RESENDS: u8 = 3;

pub(crate) trait Responder: Send + Sync {
    fn accept(&self, from: DeviceId, id: TransferId, policy: DestinationPolicy) -> BoxFut<'_>;
    fn reject(&self, from: DeviceId, id: TransferId) -> BoxFut<'_>;
    fn pause(&self, id: TransferId) -> BoxFut<'_>;
    fn resume(&self, id: TransferId) -> BoxFut<'_>;
    fn cancel(&self, id: TransferId) -> BoxFut<'_>;
}

/// §202 receiving: `let offer = files.next_offer().await?;
/// offer.accept(policy).await?`.
pub struct IncomingOffer {
    pub from: DeviceId,
    pub offer: FileOffer,
    display_name: Option<String>,
    key_status: KeyStatus,
    responder: Arc<dyn Responder>,
}

impl IncomingOffer {
    /// Whether this device will be able to open the file once received.
    pub fn key_status(&self) -> KeyStatus {
        self.key_status
    }

    /// The sender's name for the file, if it sent one. Display only;
    /// untrusted.
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// §144: the trusted facts an accept/decline sheet may show.
    pub fn prompt(&self) -> AcceptancePrompt {
        AcceptancePrompt::new(self.from, &self.offer)
    }

    pub async fn accept(self, policy: DestinationPolicy) -> Result<(), FilesError> {
        self.responder
            .accept(self.from, self.offer.transfer_id, policy)
            .await
    }

    pub async fn reject(self) -> Result<(), FilesError> {
        self.responder
            .reject(self.from, self.offer.transfer_id)
            .await
    }
}

/// §202 sending: `transfer.pause().await?` etc.
pub struct SendHandle {
    transfer_id: TransferId,
    responder: Arc<dyn Responder>,
}

impl SendHandle {
    pub fn transfer_id(&self) -> TransferId {
        self.transfer_id
    }

    /// Stops serving chunks; the receiver's own state is untouched.
    pub async fn pause(&self) -> Result<(), FilesError> {
        self.responder.pause(self.transfer_id).await
    }

    pub async fn resume(&self) -> Result<(), FilesError> {
        self.responder.resume(self.transfer_id).await
    }

    /// Tells the receiver and releases the local reference.
    pub async fn cancel(&self) -> Result<(), FilesError> {
        self.responder.cancel(self.transfer_id).await
    }
}

pub struct FilesService<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    engine: Arc<TransferEngine<M, R>>,
    refs: Arc<R>,
    channel: C,
    namespace: NamespaceId,
    limits: WireLimits,
    caps: FilesCapabilities,
    receiver: FileReceiver<M, R>,
    peers: Mutex<HashMap<DeviceId, NegotiatedFiles>>,
    outgoing: Mutex<HashMap<TransferId, Outgoing>>,
    incoming_from: Mutex<HashMap<TransferId, DeviceId>>,
    accepted: Mutex<HashMap<TransferId, ReceiveContext>>,
    origin: EventOrigin,
    offers_tx: mpsc::Sender<IncomingOffer>,
    offers_rx: tokio::sync::Mutex<mpsc::Receiver<IncomingOffer>>,
    app: AppNamespace,
    sched: Mutex<FairScheduler>,
    pumping: AtomicBool,
    gc: GcScheduler,
    gc_grace_millis: u64,
    plaintext: Mutex<PlaintextCache>,
    notifier: Option<Arc<dyn NotificationSink>>,
    auto_download: Option<AutoDownload>,
    network: Mutex<NetworkKind>,
    paths: Mutex<HashMap<DeviceId, PathKind>>,
    resources: Mutex<ResourceContext>,
    adaptive: Mutex<HashMap<DeviceId, AdaptiveConcurrency>>,
    adaptive_enabled: bool,
    stall_millis: u64,
    /// Last time (ms) each incoming transfer made progress.
    progress: Mutex<HashMap<TransferId, u64>>,
    serve_gate: Option<Arc<dyn ServeGate>>,
    fetch_timeout_millis: u64,
    fetch_first_response_millis: u64,
    pairwise: Option<Arc<dyn PairwiseSecrets>>,
    ledger: Arc<dyn AccessLedger>,
    local_device: Option<DeviceId>,
    /// Blobs this device has asked a peer for (pull), awaiting arrival.
    fetches: Mutex<HashMap<BlobId, Fetch>>,
    /// Which blob each incoming transfer carries (to resolve a fetch).
    incoming_blob: Mutex<HashMap<TransferId, BlobId>>,
    /// Key and name from each incoming offer, until the transfer ends.
    incoming_meta: Mutex<HashMap<TransferId, IncomingMeta>>,
    manifest_cache: Mutex<ManifestLru>,
    received_tx: mpsc::UnboundedSender<ReceivedFile>,
    received_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<ReceivedFile>>,
}

/// Descriptors and manifests rebuilt for pulls. A blob's manifest never
/// changes (it is derived from content-addressed bytes), so a hit is valid
/// for as long as the blob is still stored — checked on every use, never
/// assumed, and pruned in [`FilesService::maintenance`].
type ManifestLru = Lru<
    BlobId,
    (
        siar_blob_manifest::descriptor::BlobDescriptor,
        Arc<siar_blob_manifest::manifest::BlobManifest>,
    ),
>;

/// Entries kept; manifests of large blobs are big, so this stays small.
const MANIFEST_CACHE_ENTRIES: usize = 16;

struct IncomingMeta {
    meta: OfferMetadata,
    from: DeviceId,
    size: u64,
    media_type: Option<MediaType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchOutcome {
    Done,
    Unavailable,
    Failed,
}

struct Fetch {
    from: DeviceId,
    dest: DestinationPolicy,
    /// The peer has reacted (its offer for this blob arrived).
    responded: bool,
    waiters: Vec<oneshot::Sender<FetchOutcome>>,
}

impl<C, M, R> FilesService<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    pub fn new(
        engine: Arc<TransferEngine<M, R>>,
        refs: Arc<R>,
        channel: C,
        cfg: FilesConfig,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(cfg.offer_queue.max(1));
        // Unbounded on purpose: a `ReceivedFile` carries the only copy of
        // the key, so it must never be dropped for want of room.
        let (received_tx, received_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            receiver: FileReceiver::new(Arc::clone(&engine), cfg.limits, cfg.capabilities),
            caps: cfg.capabilities.clamped_to(&cfg.limits),
            engine,
            refs,
            channel,
            namespace: cfg.namespace,
            limits: cfg.limits,
            peers: Mutex::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            incoming_from: Mutex::new(HashMap::new()),
            accepted: Mutex::new(HashMap::new()),
            origin: cfg.origin,
            offers_tx: tx,
            offers_rx: tokio::sync::Mutex::new(rx),
            app: cfg.app,
            sched: Mutex::new(FairScheduler::new(cfg.scheduler)),
            pumping: AtomicBool::new(false),
            gc: GcScheduler::new(cfg.gc_interval_millis, 256),
            gc_grace_millis: cfg.gc_grace_millis,
            plaintext: Mutex::new(PlaintextCache::new(cfg.plaintext)),
            notifier: cfg.notifier,
            auto_download: cfg.auto_download,
            network: Mutex::new(NetworkKind::Unmetered),
            paths: Mutex::new(HashMap::new()),
            resources: Mutex::new(ResourceContext::default()),
            adaptive: Mutex::new(HashMap::new()),
            adaptive_enabled: cfg.adaptive_concurrency,
            stall_millis: cfg.stall_millis,
            progress: Mutex::new(HashMap::new()),
            serve_gate: cfg.serve_gate,
            fetch_timeout_millis: cfg.fetch_timeout_millis,
            fetch_first_response_millis: cfg.fetch_first_response_millis,
            pairwise: cfg.pairwise_secrets,
            ledger: cfg
                .access_ledger
                .unwrap_or_else(|| Arc::new(MemoryAccessLedger::new())),
            local_device: match cfg.origin {
                EventOrigin::LocalDevice(d) => Some(d),
                _ => None,
            },
            fetches: Mutex::new(HashMap::new()),
            incoming_blob: Mutex::new(HashMap::new()),
            incoming_meta: Mutex::new(HashMap::new()),
            manifest_cache: Mutex::new(ManifestLru::new(MANIFEST_CACHE_ENTRIES)),
            received_tx,
            received_rx: tokio::sync::Mutex::new(received_rx),
        })
    }

    pub fn engine(&self) -> &Arc<TransferEngine<M, R>> {
        &self.engine
    }

    /// The host reports connectivity so the auto-download policy can
    /// tell Wi-Fi from a metered link.
    pub fn set_network_kind(&self, kind: NetworkKind) {
        *self.network.lock().expect("network") = kind;
    }

    /// §42: which kind of path currently carries this peer. Bluetooth
    /// gets one chunk in flight, a LAN several.
    pub fn set_path_kind(&self, peer: DeviceId, kind: PathKind) {
        self.paths.lock().expect("paths").insert(peer, kind);
        self.apply_peer_cap(peer);
    }

    /// §41: what the device is under (battery saver, memory pressure…).
    pub fn set_resources(&self, ctx: ResourceContext) {
        *self.resources.lock().expect("resources") = ctx;
        let peers: Vec<DeviceId> = self.peers.lock().expect("peers").keys().copied().collect();
        for p in peers {
            self.apply_peer_cap(p);
        }
    }

    /// §43: report an observation window for a peer. Returns the
    /// concurrency now allowed; a no-op returning the deterministic limit
    /// unless `adaptive_concurrency` was enabled.
    pub fn report_sample(self: &Arc<Self>, peer: DeviceId, sample: Sample) -> u16 {
        let limit = if self.adaptive_enabled {
            let ceiling = self.deterministic_limit(peer);
            let mut map = self.adaptive.lock().expect("adaptive");
            let a = map
                .entry(peer)
                .or_insert_with(|| AdaptiveConcurrency::new(ceiling));
            a.set_ceiling(ceiling);
            a.on_sample(sample)
        } else {
            self.deterministic_limit(peer)
        };
        self.sched
            .lock()
            .expect("sched")
            .set_peer_cap(peer, usize::from(limit));
        self.kick();
        limit
    }

    fn deterministic_limit(&self, peer: DeviceId) -> u16 {
        let path = self
            .paths
            .lock()
            .expect("paths")
            .get(&peer)
            .copied()
            .unwrap_or(PathKind::IrohRelay);
        let ctx = *self.resources.lock().expect("resources");
        let negotiated = self
            .peers
            .lock()
            .expect("peers")
            .get(&peer)
            .map_or(1, |n| n.parallelism);
        effective_parallelism(path, ctx, negotiated)
    }

    fn apply_peer_cap(&self, peer: DeviceId) {
        let det = self.deterministic_limit(peer);
        let cap = if self.adaptive_enabled {
            let mut map = self.adaptive.lock().expect("adaptive");
            let a = map
                .entry(peer)
                .or_insert_with(|| AdaptiveConcurrency::new(det));
            a.set_ceiling(det);
            a.limit()
        } else {
            det
        };
        self.sched
            .lock()
            .expect("sched")
            .set_peer_cap(peer, usize::from(cap));
    }

    fn notify(&self, transfer: TransferId, what: TransferNotification) {
        if let Some(n) = &self.notifier {
            n.notify(transfer, what);
        }
    }

    /// A cached description of `blob`, if one exists *and the blob is
    /// still stored* — a stale entry is evicted, never served. A hit
    /// refreshes the entry's recency.
    pub(crate) fn cached_description(
        &self,
        blob: BlobId,
    ) -> Option<(
        siar_blob_manifest::descriptor::BlobDescriptor,
        Arc<siar_blob_manifest::manifest::BlobManifest>,
    )> {
        let mut c = self.manifest_cache.lock().expect("manifest cache");
        let hit = c.get(&blob)?;
        if !self.engine.store().contains_sync(blob) {
            c.remove(&blob);
            return None;
        }
        Some(hit)
    }

    pub(crate) fn cache_description(
        &self,
        blob: BlobId,
        descriptor: siar_blob_manifest::descriptor::BlobDescriptor,
        manifest: Arc<siar_blob_manifest::manifest::BlobManifest>,
    ) {
        self.manifest_cache
            .lock()
            .expect("manifest cache")
            .insert(blob, (descriptor, manifest));
    }

    /// Drops cached manifests whose blob is no longer stored (GC just
    /// reclaimed it), so they stop occupying memory instead of waiting
    /// for someone to ask for them. Called from [`Self::maintenance`].
    pub fn prune_manifest_cache(&self) {
        let store = self.engine.store();
        self.manifest_cache
            .lock()
            .expect("manifest cache")
            .retain(|b| store.contains_sync(*b));
    }

    /// How many rebuilt manifests are cached (diagnostics and tests).
    pub fn cached_manifests(&self) -> usize {
        self.manifest_cache.lock().expect("manifest cache").len()
    }

    pub(crate) fn now_millis(&self) -> u64 {
        now_millis()
    }

    pub(crate) fn plaintext_get(&self, id: BlobId, now: u64) -> Option<Vec<u8>> {
        self.plaintext
            .lock()
            .expect("plaintext")
            .get(id, now)
            .map(<[u8]>::to_vec)
    }

    pub(crate) fn plaintext_put(&self, id: BlobId, bytes: &[u8], now: u64) {
        self.plaintext
            .lock()
            .expect("plaintext")
            .insert(id, bytes.to_vec(), now);
    }

    /// Wipes every cached plaintext (lock screen, logout, memory
    /// warning) and, for storage pressure, [`Self::shrink_plaintext_cache`].
    pub fn clear_plaintext_cache(&self) {
        self.plaintext.lock().expect("plaintext").clear();
    }

    pub fn shrink_plaintext_cache(&self, target_bytes: u64) {
        self.plaintext
            .lock()
            .expect("plaintext")
            .shrink_to(target_bytes);
    }

    pub(crate) fn refs(&self) -> &R {
        &self.refs
    }

    pub(crate) fn namespace(&self) -> &NamespaceId {
        &self.namespace
    }

    pub(crate) fn limits(&self) -> &WireLimits {
        &self.limits
    }

    async fn send_control(&self, to: DeviceId, msg: &FileControlV1) -> Result<(), FilesError> {
        let frame = encode_control(msg)?;
        self.channel
            .send_frame(to, FrameKind::Control, frame)
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Sending
    // ------------------------------------------------------------------

    /// §205: send a file to a device. No conversation involved.
    pub async fn send_file<S: FileSource + ?Sized>(
        self: &Arc<Self>,
        dest: Destination,
        source: &S,
        policy: FileSendPolicy,
    ) -> Result<SendHandle, FilesError> {
        let imported = self.import_attachment(source, policy.media_type).await?;
        let metadata =
            self.offer_metadata(&imported.key, &policy, imported.blob_id, dest.device())?;
        let object = imported.import_owner.clone();
        let blob = BlobToSend {
            descriptor: imported.descriptor,
            blob_id: imported.blob_id,
            manifest: Arc::clone(&imported.manifest),
            metadata,
        };
        self.offer_imported(dest, blob, object, policy).await
    }

    /// Sends a blob that was already imported (typically one bound to a
    /// message): the same blob can be offered again — to another device,
    /// or again to the same one after a lost acknowledgement — without
    /// re-encrypting it. Holds its own reference for the duration of the
    /// transfer, so the message's reference is never touched.
    pub async fn send_imported(
        self: &Arc<Self>,
        dest: Destination,
        imported: &crate::ImportedAttachment,
        policy: FileSendPolicy,
    ) -> Result<SendHandle, FilesError> {
        let metadata =
            self.offer_metadata(&imported.key, &policy, imported.blob_id, dest.device())?;
        let blob = BlobToSend {
            descriptor: imported.descriptor,
            blob_id: imported.blob_id,
            manifest: Arc::clone(&imported.manifest),
            metadata,
        };
        self.offer_with_own_ref(dest, blob, policy).await
    }

    /// Offers a stored blob under a reference of the transfer's own, so
    /// neither the message's reference nor GC can pull it away mid-send;
    /// the reference is undone if the offer cannot be sent.
    async fn offer_with_own_ref(
        self: &Arc<Self>,
        dest: Destination,
        blob: BlobToSend,
        policy: FileSendPolicy,
    ) -> Result<SendHandle, FilesError> {
        let blob_id = blob.blob_id;
        let object = ObjectId::new(&format!("send-{}", uuid::Uuid::new_v4()))?;
        let reference = self.reference_for(&object, blob_id, BlobRole::Transfer);
        self.refs.apply(&[RefOp::Add(reference.clone())])?;
        let sent = self.offer_imported(dest, blob, object, policy).await;
        if sent.is_err() {
            if let Err(e) = self.refs.apply(&[RefOp::Remove(reference)]) {
                tracing::warn!(error = %e, "could not undo a send reference");
            }
        }
        sent
    }

    // ------------------------------------------------------------------
    // Pull: asking a peer for a blob by id
    // ------------------------------------------------------------------

    /// Asks `from` for `blob` and waits (up to `fetch_timeout_millis`)
    /// until it has arrived, verified and referenced under `dest`.
    /// Returns at once if the blob is already stored here.
    ///
    /// The holder answers with an ordinary offer, which this device
    /// accepts automatically because it asked for it. Everything an
    /// ordinary transfer guarantees still applies: the manifest must be
    /// the requested blob's, every chunk and the whole blob are hashed,
    /// and a wrong blob can never be accepted under the requested id.
    pub async fn fetch_blob(
        self: &Arc<Self>,
        from: Destination,
        blob: BlobId,
        dest: DestinationPolicy,
    ) -> Result<(), FilesError> {
        if self.engine.store().contains_sync(blob) {
            return Ok(());
        }
        let to = from.device();
        let (tx, rx) = oneshot::channel();
        // The first caller for a blob sends the request; later callers
        // for the same blob from the same peer just wait on that one
        // transfer (a second request would make the holder offer it twice).
        let first = {
            let mut fetches = self.fetches.lock().expect("fetches");
            match fetches.get_mut(&blob) {
                Some(f) if f.from == to => {
                    f.waiters.push(tx);
                    false
                }
                // Already being fetched from someone else: do not race two
                // peers to fill the same destination.
                Some(_) => return Err(FilesError::Unexpected),
                None => {
                    fetches.insert(
                        blob,
                        Fetch {
                            from: to,
                            dest,
                            responded: false,
                            waiters: vec![tx],
                        },
                    );
                    true
                }
            }
        };
        if first {
            let sent = async {
                self.send_control(to, &FileControlV1::Capabilities(self.caps))
                    .await?;
                self.send_control(to, &FileControlV1::BlobRequest { blob_id: blob })
                    .await
            }
            .await;
            if let Err(e) = sent {
                // Wake everyone who joined: the request never left.
                self.resolve_fetch(blob, FetchOutcome::Failed);
                return Err(e);
            }
        }
        let overall = std::time::Duration::from_millis(self.fetch_timeout_millis);
        let first = std::time::Duration::from_millis(self.fetch_first_response_millis);
        let started = tokio::time::Instant::now();
        let mut rx = rx;
        let first_deadline = tokio::time::sleep(first);
        tokio::pin!(first_deadline);
        let mut first_checked = false;
        let result = loop {
            tokio::select! {
                r = &mut rx => break Some(r),
                () = &mut first_deadline, if !first_checked => {
                    first_checked = true;
                    let responded = self
                        .fetches
                        .lock()
                        .expect("fetches")
                        .get(&blob)
                        .is_none_or(|f| f.responded);
                    if !responded {
                        // Silence, not slowness: give up now so a fallback
                        // can run instead of waiting out the whole fetch.
                        self.resolve_fetch(blob, FetchOutcome::Failed);
                        return Err(FilesError::NoResponse);
                    }
                }
                () = tokio::time::sleep_until(started + overall) => break None,
            }
        };
        match result {
            Some(Ok(FetchOutcome::Done)) => Ok(()),
            Some(Ok(FetchOutcome::Unavailable)) => Err(FilesError::BlobUnavailable),
            Some(Ok(FetchOutcome::Failed)) | Some(Err(_)) => Err(FilesError::FetchFailed),
            None => {
                self.fetches.lock().expect("fetches").remove(&blob);
                Err(FilesError::FetchTimeout)
            }
        }
    }

    /// [`Self::fetch_blob`] for a message's attachment reference, then
    /// the decrypted bytes.
    pub async fn fetch_attachment(
        self: &Arc<Self>,
        from: Destination,
        reference: &siar_domain::AttachmentReference,
        dest: DestinationPolicy,
    ) -> Result<Vec<u8>, FilesError> {
        let blob = BlobId::from_reference_bytes(reference.blob_hash);
        self.fetch_blob(from, blob, dest).await?;
        self.open_attachment(reference).await
    }

    /// A transfer carrying `blob` failed or was cancelled. The fetch only
    /// fails if no *other* transfer for the same blob is still in flight
    /// (a redundant duplicate failing must not sink the one that works).
    fn fail_fetch_unless_another_transfer(&self, blob: BlobId) {
        let another = self
            .incoming_blob
            .lock()
            .expect("incoming blob")
            .values()
            .any(|b| *b == blob);
        if !another {
            self.resolve_fetch(blob, FetchOutcome::Failed);
        }
    }

    /// Turns an offer's metadata field into a usable key (when there is
    /// one this device can open), a display name, and an honest status.
    fn resolve_metadata(
        &self,
        from: DeviceId,
        blob: BlobId,
        bytes: &[u8],
    ) -> (
        Option<OfferMetadata>,
        Option<siar_blob_manifest::descriptor::FileName>,
        KeyStatus,
    ) {
        match offer_meta::decode_any(bytes) {
            None => (None, None, KeyStatus::Absent),
            Some(Decoded::Key(m)) => {
                let name = m.file_name.clone();
                (Some(m), name, KeyStatus::Present)
            }
            Some(Decoded::Wrapped { wrapped, file_name }) => {
                let key = self
                    .pairwise
                    .as_ref()
                    .and_then(|p| p.secret_for(from))
                    .zip(self.local_device)
                    .and_then(|(secret, me)| unwrap_blob_key(&wrapped, &secret, blob, me).ok());
                match key {
                    Some(key) => (
                        Some(OfferMetadata {
                            key,
                            file_name: file_name.clone(),
                        }),
                        file_name,
                        KeyStatus::Present,
                    ),
                    None => (None, file_name, KeyStatus::WrapUnopenable),
                }
            }
        }
    }

    /// The metadata bytes for one recipient, per the policy's key delivery.
    fn offer_metadata(
        &self,
        key: &BlobEncryptionKey,
        policy: &FileSendPolicy,
        blob: BlobId,
        to: DeviceId,
    ) -> Result<Vec<u8>, FilesError> {
        if self.ledger.is_revoked(blob, to) {
            return Err(FilesError::AccessRevoked);
        }
        // Recorded before the key leaves, so a revocation made while the
        // offer is in flight still reports that a key may be held.
        self.ledger.record_grant(blob, to);
        match policy.key_delivery {
            KeyDelivery::InOffer => offer_meta::encode(key, policy.file_name.as_deref()),
            KeyDelivery::Wrapped => {
                let secrets = self
                    .pairwise
                    .as_ref()
                    .ok_or(FilesError::NoPairwiseSecret(to))?;
                let secret = secrets
                    .secret_for(to)
                    .ok_or(FilesError::NoPairwiseSecret(to))?;
                // The wrap is bound to the *recipient's* device id: the
                // receiving side opens it with its own.
                let wrapped =
                    wrap_blob_key(key, &secret, blob, to).map_err(|_| FilesError::WrapFailed)?;
                offer_meta::encode_wrapped(&wrapped, policy.file_name.as_deref())
            }
        }
    }

    /// A transfer finished: hand the application the file and its key.
    /// Transfers that came without metadata (a pull, where the requester
    /// already holds the reference) produce no event.
    fn deliver_received(&self, id: TransferId, blob: BlobId) {
        let meta = self
            .incoming_meta
            .lock()
            .expect("incoming meta")
            .remove(&id);
        if let Some(m) = meta {
            let _ = self.received_tx.send(ReceivedFile {
                transfer_id: id,
                from: m.from,
                blob_id: blob,
                stored_size: m.size,
                media_type: m.media_type,
                file_name: m.meta.file_name,
                key: m.meta.key,
            });
        }
    }

    /// The next file that arrived and verified, with its key (§202).
    pub async fn next_received(&self) -> Option<ReceivedFile> {
        self.received_rx.lock().await.recv().await
    }

    fn resolve_fetch(&self, blob: BlobId, outcome: FetchOutcome) {
        let fetch = self.fetches.lock().expect("fetches").remove(&blob);
        if let Some(f) = fetch {
            for w in f.waiters {
                let _ = w.send(outcome);
            }
        }
    }

    /// A peer asked for a blob this device may hold.
    async fn on_blob_request(
        self: &Arc<Self>,
        from: DeviceId,
        blob: BlobId,
    ) -> Result<(), FilesError> {
        if !self.peers.lock().expect("peers").contains_key(&from) {
            return Err(FilesError::NoCapabilities(from));
        }
        let revoked = self.ledger.is_revoked(blob, from);
        let allowed = !revoked
            && match &self.serve_gate {
                Some(g) => g.may_serve(from, blob),
                None => self.refs.count_for(blob).map(|c| c > 0).unwrap_or(false),
            };
        let described = if allowed {
            self.describe_stored_blob(blob).await.ok()
        } else {
            None
        };
        // Refusing, missing and unreadable all look the same to the asker.
        let Some((descriptor, manifest)) = described else {
            return self
                .send_control(from, &FileControlV1::BlobUnavailable { blob_id: blob })
                .await;
        };
        let blob = BlobToSend {
            descriptor,
            blob_id: blob,
            manifest,
            // The requester already holds the key in its reference.
            metadata: Vec::new(),
        };
        self.offer_with_own_ref(Destination::Device(from), blob, FileSendPolicy::default())
            .await
            .map(|_| ())
    }

    /// One import, many recipients. The file is encrypted and stored
    /// **once**; each recipient is offered that same blob with a key
    /// wrapped for them alone, so a gigabyte sent to fifty devices is one
    /// blob on disk and fifty 60-byte wraps, not fifty copies.
    ///
    /// Requires [`KeyDelivery::Wrapped`] (the raw key must never be sent
    /// to several parties in the clear). Recipients are independent: one
    /// with no pairwise secret, or an unreachable one, fails on its own
    /// and the others still go out. Each transfer holds its own
    /// reference, and the temporary import reference is released when
    /// the offers are out, so the blob lives exactly as long as some
    /// transfer (or a message the caller binds it to) needs it.
    pub async fn send_file_to_many<S: FileSource + ?Sized>(
        self: &Arc<Self>,
        dests: &[Destination],
        source: &S,
        policy: FileSendPolicy,
    ) -> Result<Vec<(Destination, Result<SendHandle, FilesError>)>, FilesError> {
        if policy.key_delivery != KeyDelivery::Wrapped {
            return Err(FilesError::KeyMustBeWrapped);
        }
        let imported = self.import_attachment(source, policy.media_type).await?;
        Ok(self.fan_out(dests, imported, policy).await)
    }

    /// [`Self::send_file_to_many`] with the chunk-wise (v2) format and
    /// bounded memory: the file is read twice in segments and never held
    /// whole, however many recipients there are. Every recipient must
    /// understand v2 (an older peer refuses the offer cleanly, since the
    /// algorithm is part of it).
    pub async fn send_file_to_many_streaming<S: FileSource + ?Sized>(
        self: &Arc<Self>,
        dests: &[Destination],
        source: &S,
        policy: FileSendPolicy,
    ) -> Result<Vec<(Destination, Result<SendHandle, FilesError>)>, FilesError> {
        if policy.key_delivery != KeyDelivery::Wrapped {
            return Err(FilesError::KeyMustBeWrapped);
        }
        let imported = self
            .import_attachment_streaming(source, policy.media_type)
            .await?;
        Ok(self.fan_out(dests, imported, policy).await)
    }

    /// Offers one imported blob to every destination, each with its own
    /// wrapped key and its own reference, then releases the import's.
    async fn fan_out(
        self: &Arc<Self>,
        dests: &[Destination],
        imported: crate::ImportedAttachment,
        policy: FileSendPolicy,
    ) -> Vec<(Destination, Result<SendHandle, FilesError>)> {
        let mut results = Vec::with_capacity(dests.len());
        for &dest in dests {
            let sent = match self.offer_metadata(
                &imported.key,
                &policy,
                imported.blob_id,
                dest.device(),
            ) {
                Err(e) => Err(e),
                Ok(metadata) => {
                    let blob = BlobToSend {
                        descriptor: imported.descriptor,
                        blob_id: imported.blob_id,
                        manifest: Arc::clone(&imported.manifest),
                        metadata,
                    };
                    self.offer_with_own_ref(dest, blob, policy.clone()).await
                }
            };
            results.push((dest, sent));
        }
        let import_ref =
            self.reference_for(&imported.import_owner, imported.blob_id, BlobRole::Transfer);
        if let Err(e) = self.refs.apply(&[RefOp::Remove(import_ref)]) {
            tracing::warn!(error = %e, "could not release the import reference");
        }
        results
    }

    /// [`Self::send_file`] with the chunk-wise (v2) format and bounded
    /// memory. The receiver must understand v2; an older peer refuses the
    /// offer cleanly (the algorithm is part of it).
    pub async fn send_file_streaming<S: FileSource + ?Sized>(
        self: &Arc<Self>,
        dest: Destination,
        source: &S,
        policy: FileSendPolicy,
    ) -> Result<SendHandle, FilesError> {
        let imported = self
            .import_attachment_streaming(source, policy.media_type)
            .await?;
        let metadata =
            self.offer_metadata(&imported.key, &policy, imported.blob_id, dest.device())?;
        let object = imported.import_owner.clone();
        let blob = BlobToSend {
            descriptor: imported.descriptor,
            blob_id: imported.blob_id,
            manifest: Arc::clone(&imported.manifest),
            metadata,
        };
        self.offer_imported(dest, blob, object, policy).await
    }

    async fn offer_imported(
        self: &Arc<Self>,
        dest: Destination,
        blob: BlobToSend,
        object: ObjectId,
        policy: FileSendPolicy,
    ) -> Result<SendHandle, FilesError> {
        let BlobToSend {
            descriptor,
            blob_id,
            manifest,
            metadata,
        } = blob;
        let to = dest.device();
        let transfer_id = TransferId::new();
        let offer = FileOffer {
            transfer_id,
            blob: descriptor,
            encrypted_metadata: metadata,
            expiry_millis: policy.expiry_millis,
        };
        let offer_frame = encode_control(&FileControlV1::Offer(FileOfferV1::from(&offer)))?;
        self.outgoing.lock().expect("outgoing").insert(
            transfer_id,
            Outgoing {
                to,
                blob_id,
                manifest,
                object,
                class: policy.class,
                paused: false,
                queue: VecDeque::new(),
                sender: None,
                offer_frame: offer_frame.clone(),
                offered_at_millis: now_millis(),
                answered: false,
                resends: 0,
            },
        );
        let sent = async {
            self.send_control(to, &FileControlV1::Capabilities(self.caps))
                .await?;
            self.channel
                .send_frame(to, FrameKind::Control, offer_frame)
                .await?;
            Ok::<(), FilesError>(())
        }
        .await;
        if let Err(e) = sent {
            self.drop_outgoing(transfer_id);
            return Err(e);
        }
        Ok(SendHandle {
            transfer_id,
            responder: Arc::clone(self) as Arc<dyn Responder>,
        })
    }

    /// Forgets a sent transfer and releases its import reference.
    fn drop_outgoing(&self, id: TransferId) {
        let removed = self.outgoing.lock().expect("outgoing").remove(&id);
        self.sched.lock().expect("sched").remove(id);
        if let Some(o) = removed {
            let r = self.reference_for(&o.object, o.blob_id, BlobRole::Transfer);
            if let Err(e) = self.refs.apply(&[RefOp::Remove(r)]) {
                tracing::warn!(error = %e, "could not release a sent transfer's reference");
            }
        }
    }

    // ------------------------------------------------------------------
    // Receiving
    // ------------------------------------------------------------------

    /// §202: the next offer for the application to answer.
    pub async fn next_offer(&self) -> Result<IncomingOffer, FilesError> {
        self.offers_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(FilesError::OfferQueueClosed)
    }

    /// The receiving side's own pause/resume/cancel (§102, §31, §101).
    pub async fn pause_incoming(&self, id: TransferId) -> Result<(), FilesError> {
        let to = self.incoming_peer(id)?;
        let msg = self.receiver.pause(id).await?;
        self.send_control(to, &msg).await
    }

    pub async fn resume_incoming(&self, id: TransferId) -> Result<(), FilesError> {
        let to = self.incoming_peer(id)?;
        let msg = self.receiver.resume(id).await?;
        self.send_control(to, &msg).await
    }

    pub async fn cancel_incoming(&self, id: TransferId) -> Result<(), FilesError> {
        let to = self.incoming_peer(id)?;
        let msg = self.receiver.cancel(id).await?;
        self.incoming_from.lock().expect("incoming").remove(&id);
        self.incoming_meta
            .lock()
            .expect("incoming meta")
            .remove(&id);
        self.send_control(to, &msg).await
    }

    fn touch_progress(&self, id: TransferId) {
        self.progress
            .lock()
            .expect("progress")
            .insert(id, now_millis());
    }

    /// Re-requests every receiving transfer that has made no progress for
    /// `stall_millis`: the missing chunk ranges if the manifest arrived,
    /// the manifest itself if it did not. Idempotent and safe to call on
    /// a timer ([`Self::spawn_maintenance`] does). Returns how many
    /// transfers were nudged.
    ///
    /// (The sender-side mirror — re-sending an unanswered offer — is a
    /// transport/session concern and is not done here.)
    pub async fn retry_stalled(self: &Arc<Self>, now_millis: u64) -> Result<usize, FilesError> {
        let stalled: Vec<(TransferId, DeviceId)> = {
            let progress = self.progress.lock().expect("progress");
            let incoming = self.incoming_from.lock().expect("incoming");
            incoming
                .iter()
                .filter(|(id, _)| {
                    progress
                        .get(id)
                        .is_some_and(|t| now_millis.saturating_sub(*t) >= self.stall_millis)
                })
                .map(|(id, from)| (*id, *from))
                .collect()
        };
        // Forget progress for transfers that no longer exist.
        {
            let incoming = self.incoming_from.lock().expect("incoming");
            self.progress
                .lock()
                .expect("progress")
                .retain(|id, _| incoming.contains_key(id));
        }
        let mut nudged = 0;
        for (id, from) in stalled {
            let msg = match self.receiver.request_missing(id) {
                Ok(m) => m,
                Err(_) => match self.receiver.request_manifest_again(id) {
                    Ok(m) => m,
                    Err(_) => continue, // finished or gone in the meantime
                },
            };
            if let FileControlV1::ChunkRequest(r) = &msg {
                if r.ranges.is_empty() {
                    continue; // everything is here; finishing is in flight
                }
            }
            self.send_control(from, &msg).await?;
            self.progress
                .lock()
                .expect("progress")
                .insert(id, now_millis);
            nudged += 1;
        }
        Ok(nudged)
    }

    /// Sender-side mirror of [`Self::retry_stalled`]: an offer nobody has
    /// answered within `stall_millis` is sent again (capabilities first,
    /// then the identical offer). A receiver that already knows the
    /// transfer ignores the repeat; one that lost it — a crash, a dropped
    /// frame, an app that was not running — sees it fresh. After
    /// [`MAX_OFFER_RESENDS`] unanswered repeats the transfer is given up
    /// and its reference released. Returns how many offers were re-sent.
    pub async fn retry_unanswered_offers(
        self: &Arc<Self>,
        now_millis: u64,
    ) -> Result<usize, FilesError> {
        let mut resend: Vec<(TransferId, DeviceId, Vec<u8>)> = Vec::new();
        let mut give_up: Vec<TransferId> = Vec::new();
        {
            let mut out = self.outgoing.lock().expect("outgoing");
            for (id, o) in out.iter_mut() {
                if o.answered || now_millis.saturating_sub(o.offered_at_millis) < self.stall_millis
                {
                    continue;
                }
                if o.resends >= MAX_OFFER_RESENDS {
                    give_up.push(*id);
                } else {
                    o.resends += 1;
                    o.offered_at_millis = now_millis;
                    resend.push((*id, o.to, o.offer_frame.clone()));
                }
            }
        }
        for id in give_up {
            tracing::debug!(transfer = %id, "offer never answered; giving the transfer up");
            self.drop_outgoing(id);
        }
        let mut sent = 0;
        for (_, to, frame) in resend {
            self.send_control(to, &FileControlV1::Capabilities(self.caps))
                .await?;
            self.channel
                .send_frame(to, FrameKind::Control, frame)
                .await?;
            sent += 1;
        }
        Ok(sent)
    }

    fn incoming_peer(&self, id: TransferId) -> Result<DeviceId, FilesError> {
        self.incoming_from
            .lock()
            .expect("incoming")
            .get(&id)
            .copied()
            .ok_or(FilesError::UnknownTransfer)
    }

    // ------------------------------------------------------------------
    // Inbound
    // ------------------------------------------------------------------

    /// The one entry point for the host's receive loop.
    pub async fn handle_frame(
        self: &Arc<Self>,
        from: DeviceId,
        kind: u8,
        frame: &[u8],
    ) -> Result<(), FilesError> {
        match FrameKind::from_u8(kind).ok_or(FilesError::UnknownFrameKind(kind))? {
            FrameKind::Control => {
                let msg = decode_control(frame, &self.limits)?;
                self.on_control(from, msg).await
            }
            FrameKind::Manifest => self.on_manifest(from, frame).await,
            FrameKind::Chunk => self.on_chunk(from, frame).await,
        }
    }

    async fn on_control(
        self: &Arc<Self>,
        from: DeviceId,
        msg: FileControlV1,
    ) -> Result<(), FilesError> {
        match msg {
            FileControlV1::Capabilities(c) => {
                let neg = self.receiver.negotiate_with(&c)?;
                let first = self
                    .peers
                    .lock()
                    .expect("peers")
                    .insert(from, neg)
                    .is_none();
                if first {
                    self.send_control(from, &FileControlV1::Capabilities(self.caps))
                        .await?;
                }
                Ok(())
            }
            FileControlV1::Offer(o) => self.on_offer(from, o).await,
            FileControlV1::BlobRequest { blob_id } => self.on_blob_request(from, blob_id).await,
            FileControlV1::BlobUnavailable { blob_id } => {
                let ours = self
                    .fetches
                    .lock()
                    .expect("fetches")
                    .get(&blob_id)
                    .is_some_and(|f| f.from == from);
                if ours {
                    self.resolve_fetch(blob_id, FetchOutcome::Unavailable);
                }
                Ok(())
            }
            other => {
                let Some(id) = other.transfer_id() else {
                    return Err(FilesError::Unexpected);
                };
                if self.outgoing.lock().expect("outgoing").contains_key(&id) {
                    self.on_receiver_message(from, id, other).await
                } else if self.incoming_from.lock().expect("incoming").get(&id) == Some(&from) {
                    self.receiver.on_sender_message(other).await?;
                    self.incoming_from.lock().expect("incoming").remove(&id);
                    self.incoming_meta
                        .lock()
                        .expect("incoming meta")
                        .remove(&id);
                    // Bind first: a guard created in an `if let` scrutinee lives for the whole
                    // block, and the helper locks `incoming_blob` itself.
                    let ended_blob = self
                        .incoming_blob
                        .lock()
                        .expect("incoming blob")
                        .remove(&id);
                    if let Some(b) = ended_blob {
                        self.fail_fetch_unless_another_transfer(b);
                    }
                    Ok(())
                } else {
                    Err(FilesError::UnknownTransfer)
                }
            }
        }
    }

    async fn on_offer(self: &Arc<Self>, from: DeviceId, o: FileOfferV1) -> Result<(), FilesError> {
        let neg = self
            .peers
            .lock()
            .expect("peers")
            .get(&from)
            .copied()
            .ok_or(FilesError::NoCapabilities(from))?;
        let offer = self.receiver.on_offer(o, neg, now_millis())?;
        let id = offer.transfer_id;
        self.incoming_from
            .lock()
            .expect("incoming")
            .insert(id, from);
        let blob = offer.blob.blob_id;
        self.incoming_blob
            .lock()
            .expect("incoming blob")
            .insert(id, blob);
        let (parsed, name, key_status) =
            self.resolve_metadata(from, blob, &offer.encrypted_metadata);
        let display_name = name.map(|n| n.as_str().to_owned());
        if let Some(meta) = parsed {
            self.incoming_meta.lock().expect("incoming meta").insert(
                id,
                IncomingMeta {
                    meta,
                    from,
                    size: offer.blob.size.bytes(),
                    media_type: offer.blob.media_type,
                },
            );
        }
        // A blob this device asked for is accepted without asking the
        // application again: it already said yes by requesting it.
        let wanted = self
            .fetches
            .lock()
            .expect("fetches")
            .get_mut(&blob)
            .filter(|f| f.from == from)
            .map(|f| {
                f.responded = true;
                f.dest.clone()
            });
        if let Some(dest) = wanted {
            return Responder::accept(&**self, from, id, dest).await;
        }
        // §145: the application's policy may answer for it.
        if let Some(auto) = &self.auto_download {
            let network = *self.network.lock().expect("network");
            let action = auto.policy.decide(
                offer.blob.media_type,
                offer.blob.size.bytes(),
                network,
                false,
            );
            if action == AutoAction::Download {
                let policy = (auto.destination)(&offer);
                let transfer = offer.transfer_id;
                return Responder::accept(&**self, from, transfer, policy).await;
            }
        }
        let transfer = offer.transfer_id;
        let queued = IncomingOffer {
            from,
            offer,
            display_name,
            key_status,
            responder: Arc::clone(self) as Arc<dyn Responder>,
        };
        if self.offers_tx.try_send(queued).is_err() {
            // Full (or closed): say "not now" instead of dropping it silently.
            self.incoming_from.lock().expect("incoming").remove(&id);
            self.incoming_meta
                .lock()
                .expect("incoming meta")
                .remove(&id);
            let reply = self.receiver.decide(id, FileOfferDecision::Defer)?;
            self.send_control(from, &reply).await?;
        } else {
            self.notify(transfer, TransferNotification::ActionRequired);
        }
        Ok(())
    }

    /// Messages a receiver sends about a transfer we are sending.
    async fn on_receiver_message(
        self: &Arc<Self>,
        from: DeviceId,
        id: TransferId,
        msg: FileControlV1,
    ) -> Result<(), FilesError> {
        let (to, existing) = {
            let out = self.outgoing.lock().expect("outgoing");
            let o = out.get(&id).ok_or(FilesError::UnknownTransfer)?;
            (o.to, o.sender.clone())
        };
        if from != to {
            return Err(FilesError::Unexpected); // someone else's transfer
        }
        if let Some(o) = self.outgoing.lock().expect("outgoing").get_mut(&id) {
            o.answered = true;
        }
        match msg {
            FileControlV1::OfferReply { decision, .. } => {
                if decision != siar_blob_manifest::wire::OfferDecisionV1::Accept {
                    self.drop_outgoing(id);
                }
                Ok(())
            }
            FileControlV1::ManifestRequest { .. } => {
                let neg = self
                    .peers
                    .lock()
                    .expect("peers")
                    .get(&from)
                    .copied()
                    .ok_or(FilesError::NoCapabilities(from))?;
                let (blob_id, manifest, paused) = {
                    let out = self.outgoing.lock().expect("outgoing");
                    let o = out.get(&id).ok_or(FilesError::UnknownTransfer)?;
                    (o.blob_id, Arc::clone(&o.manifest), o.paused)
                };
                let reader = {
                    use siar_blob_manifest::blob_io::BlobStore;
                    self.engine.store().open(blob_id).await?
                };
                let sender = Arc::new(FileSender::new(id, manifest, reader, &neg));
                if paused {
                    sender.on_receiver_message(&FileControlV1::Pause { transfer_id: id })?;
                }
                let frame = sender.manifest_frame()?;
                if let Some(o) = self.outgoing.lock().expect("outgoing").get_mut(&id) {
                    o.sender = Some(sender);
                }
                self.channel
                    .send_frame(from, FrameKind::Manifest, frame)
                    .await?;
                Ok(())
            }
            FileControlV1::ChunkRequest(ref req) | FileControlV1::Resume(ref req) => {
                let sender = existing.ok_or(FilesError::Unexpected)?;
                sender.on_receiver_message(&msg)?;
                self.enqueue(id, &req.ranges)?;
                self.sched.lock().expect("sched").set_paused(id, false);
                self.kick();
                Ok(())
            }
            FileControlV1::Complete(_) => {
                let sender = existing.ok_or(FilesError::Unexpected)?;
                if sender.on_receiver_message(&msg)? == SendStatus::Completed {
                    self.drop_outgoing(id);
                }
                Ok(())
            }
            FileControlV1::Cancel(_) => {
                if let Some(s) = existing {
                    s.on_receiver_message(&msg)?;
                }
                self.drop_outgoing(id);
                Ok(())
            }
            FileControlV1::Pause { .. } | FileControlV1::ChunkAck(_) => {
                existing
                    .ok_or(FilesError::Unexpected)?
                    .on_receiver_message(&msg)?;
                Ok(())
            }
            _ => Err(FilesError::Unexpected),
        }
    }

    /// A request replaces whatever was still queued: it states what the
    /// receiver is missing *now*. Registers the transfer with the fair
    /// scheduler so chunks are sent in class/peer/app-fair order.
    fn enqueue(&self, id: TransferId, ranges: &[(u32, u32)]) -> Result<(), FilesError> {
        let mut out = self.outgoing.lock().expect("outgoing");
        let o = out.get_mut(&id).ok_or(FilesError::UnknownTransfer)?;
        let total = o.manifest.chunks.len() as u64;
        let mut queue = VecDeque::new();
        for &(start, end) in ranges {
            if u64::from(end) > total {
                return Err(FilesError::Unexpected);
            }
            queue.extend(start..end);
        }
        o.queue = queue;
        let entry = self.entry_for(id, o);
        self.sched.lock().expect("sched").register(entry);
        Ok(())
    }

    fn entry_for(&self, id: TransferId, o: &Outgoing) -> SchedulerEntry {
        let next = o
            .queue
            .front()
            .and_then(|&i| o.manifest.chunks.get(i as usize))
            .map_or(0, |c| c.size);
        SchedulerEntry {
            transfer: id,
            peer: o.to,
            app: self.app,
            class: o.class,
            remaining_chunks: u32::try_from(o.queue.len()).unwrap_or(u32::MAX),
            next_chunk_bytes: next,
        }
    }

    /// Starts the pump if it is not already running.
    fn kick(self: &Arc<Self>) {
        if self
            .pumping
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let svc = Arc::clone(self);
            tokio::spawn(async move { svc.pump().await });
        }
    }

    /// Keeps as many chunk sends in flight as the scheduler allows
    /// (§41-42: per-peer caps from the path and resources, per-app and
    /// global caps from `SchedulerConfig`), granting in class/peer/app
    /// fair order, until nothing is grantable and nothing is in flight.
    async fn pump(self: Arc<Self>) {
        let mut in_flight = tokio::task::JoinSet::new();
        loop {
            loop {
                let grant = self.sched.lock().expect("sched").next_grant();
                let Some(g) = grant else { break };
                let svc = Arc::clone(&self);
                in_flight.spawn(async move { svc.send_granted(g).await });
            }
            if in_flight.is_empty() {
                self.pumping.store(false, Ordering::Release);
                // A request may have arrived between the empty grant and
                // clearing the flag: re-check once before exiting.
                let again = self.sched.lock().expect("sched").next_grant();
                let Some(g) = again else { return };
                if self
                    .pumping
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    self.sched.lock().expect("sched").abandon(g);
                    return;
                }
                let svc = Arc::clone(&self);
                in_flight.spawn(async move { svc.send_granted(g).await });
            }
            let _ = in_flight.join_next().await;
        }
    }

    async fn send_granted(&self, grant: siar_blob_manifest::scheduling::Grant) {
        let id = grant.transfer;
        let (index, sender, to) = {
            let mut out = self.outgoing.lock().expect("outgoing");
            let Some(o) = out.get_mut(&id) else {
                self.sched.lock().expect("sched").remove(id);
                return;
            };
            let (Some(index), Some(sender)) = (o.queue.pop_front(), o.sender.clone()) else {
                self.sched.lock().expect("sched").remove(id);
                return;
            };
            let entry = self.entry_for(id, o);
            self.sched
                .lock()
                .expect("sched")
                .set_next_chunk_bytes(id, entry.next_chunk_bytes);
            (index, sender, o.to)
        };
        let mut frame = Vec::new();
        let sent = match sender.serve_chunk(index, &mut frame).await {
            Ok(()) => self
                .channel
                .send_frame(to, FrameKind::Chunk, frame)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match sent {
            Ok(()) => self.sched.lock().expect("sched").complete(grant),
            Err(e) => {
                // Paused, cancelled or the link failed: put the chunk
                // back and stop granting this transfer. The receiver
                // resumes with exactly what it is missing.
                tracing::debug!(error = %e, transfer = %id, "chunk not sent");
                if let Some(o) = self.outgoing.lock().expect("outgoing").get_mut(&id) {
                    o.queue.push_front(index);
                }
                let mut s = self.sched.lock().expect("sched");
                s.abandon(grant);
                s.set_paused(id, true);
            }
        }
    }

    // ------------------------------------------------------------------
    // Maintenance (§63-65, §151)
    // ------------------------------------------------------------------

    /// One GC pass if one is due. The host calls this whenever it has a
    /// moment (or use [`FilesService::spawn_maintenance`]); in-flight
    /// transfers and referenced blobs are never collected.
    pub fn maintenance(&self, now_millis: u64) -> Result<Option<GcReport>, FilesError> {
        let policy = GcPolicy {
            grace_period_millis: self.gc_grace_millis,
        };
        let pinned: HashSet<BlobId> = HashSet::new();
        let report = self
            .gc
            .tick(&self.engine, &pinned, now_millis, &policy)
            .map_err(FilesError::Engine);
        // Whatever GC just reclaimed, its cached manifests go too.
        self.prune_manifest_cache();
        report
    }

    /// Revokes `recipient`'s access to `blob`: no further key is sent to
    /// it, pulls from it are refused (indistinguishably from "not here"),
    /// and any unfinished transfer of this blob to it is cancelled.
    ///
    /// **This stops future access only.** A key the recipient already
    /// holds, and ciphertext already delivered, cannot be recalled; the
    /// returned [`Revocation`] says whether the recipient may hold a key.
    /// Changing what *new* copies of the data protect needs a re-key
    /// (`siar_blob_manifest::key_wrapping::rekey_cost` prices it).
    pub async fn revoke_access(
        self: &Arc<Self>,
        blob: BlobId,
        recipient: Destination,
    ) -> Result<Revocation, FilesError> {
        let device = recipient.device();
        let may_hold_key = self.ledger.revoke(blob, device);
        let in_flight: Vec<TransferId> = self
            .outgoing
            .lock()
            .expect("outgoing")
            .iter()
            .filter(|(_, o)| o.blob_id == blob && o.to == device)
            .map(|(id, _)| *id)
            .collect();
        for id in in_flight {
            // Cancelling tells the peer and releases our reference. A
            // transfer that finished in the meantime is simply gone.
            match Responder::cancel(&**self, id).await {
                Ok(()) | Err(FilesError::UnknownTransfer) => {}
                Err(e) => tracing::warn!(error = %e, "could not cancel a revoked transfer"),
            }
        }
        Ok(Revocation {
            effect: RevocationEffect::StopsFutureAccessOnly,
            recipient_may_hold_key: may_hold_key,
        })
    }

    /// Lifts a revocation made with [`Self::revoke_access`].
    pub fn restore_access(&self, blob: BlobId, recipient: Destination) {
        self.ledger.restore(blob, recipient.device());
    }

    /// Runs [`FilesService::maintenance`] every `period` until the
    /// returned handle is aborted.
    pub fn spawn_maintenance(
        self: &Arc<Self>,
        period: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            loop {
                tick.tick().await;
                if let Err(e) = svc.maintenance(now_millis()) {
                    tracing::warn!(error = %e, "blob GC pass failed");
                }
                if let Err(e) = svc.retry_stalled(now_millis()).await {
                    tracing::debug!(error = %e, "stall retry failed");
                }
                if let Err(e) = svc.retry_unanswered_offers(now_millis()).await {
                    tracing::debug!(error = %e, "offer resend failed");
                }
            }
        })
    }

    async fn on_manifest(&self, from: DeviceId, frame: &[u8]) -> Result<(), FilesError> {
        let id = peek_manifest_transfer_id(frame)?;
        if self.incoming_from.lock().expect("incoming").get(&id) != Some(&from) {
            return Err(FilesError::UnknownTransfer);
        }
        let ctx = self
            .accepted
            .lock()
            .expect("accepted")
            .remove(&id)
            .ok_or(FilesError::Unexpected)?;
        let reply = match self.receiver.on_manifest(frame, ctx).await {
            Ok(r) => r,
            Err(e) => {
                self.incoming_from.lock().expect("incoming").remove(&id);
                self.incoming_meta
                    .lock()
                    .expect("incoming meta")
                    .remove(&id);
                // Bind first: a guard created in an `if let` scrutinee lives for the whole
                // block, and the helper locks `incoming_blob` itself.
                let ended_blob = self
                    .incoming_blob
                    .lock()
                    .expect("incoming blob")
                    .remove(&id);
                if let Some(b) = ended_blob {
                    self.fail_fetch_unless_another_transfer(b);
                }
                self.notify(id, TransferNotification::Failed);
                return Err(e.into());
            }
        };
        if let FileControlV1::Complete(c) = &reply {
            self.progress.lock().expect("progress").remove(&id);
            self.incoming_blob
                .lock()
                .expect("incoming blob")
                .remove(&id);
            self.notify(id, TransferNotification::Completed);
            self.deliver_received(id, c.blob_id);
            self.resolve_fetch(c.blob_id, FetchOutcome::Done);
        } else {
            self.touch_progress(id);
        }
        self.send_control(from, &reply).await
    }

    async fn on_chunk(&self, from: DeviceId, frame: &[u8]) -> Result<(), FilesError> {
        let id = peek_chunk_transfer_id(frame)?;
        if self.incoming_from.lock().expect("incoming").get(&id) != Some(&from) {
            return Err(FilesError::UnknownTransfer);
        }
        self.touch_progress(id);
        let mut cursor = frame;
        if let ChunkOutcome::Complete(msg) = self.receiver.on_chunk(id, &mut cursor).await? {
            self.incoming_from.lock().expect("incoming").remove(&id);
            self.progress.lock().expect("progress").remove(&id);
            self.incoming_blob
                .lock()
                .expect("incoming blob")
                .remove(&id);
            self.notify(id, TransferNotification::Completed);
            if let FileControlV1::Complete(c) = &msg {
                self.deliver_received(id, c.blob_id);
                self.resolve_fetch(c.blob_id, FetchOutcome::Done);
            }
            self.send_control(from, &msg).await?;
        }
        Ok(())
    }
}

impl<C, M, R> Responder for FilesService<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    fn accept(&self, from: DeviceId, id: TransferId, policy: DestinationPolicy) -> BoxFut<'_> {
        Box::pin(async move {
            let ctx = ReceiveContext {
                object: policy.object,
                role: policy.role,
                expires: policy.expires,
                origin: self.origin,
                logical_attachment_id: LogicalAttachmentId::new(),
            };
            self.accepted.lock().expect("accepted").insert(id, ctx);
            self.touch_progress(id);
            let msg = self.receiver.decide(id, FileOfferDecision::Accept)?;
            self.send_control(from, &msg).await
        })
    }

    fn reject(&self, from: DeviceId, id: TransferId) -> BoxFut<'_> {
        Box::pin(async move {
            self.incoming_from.lock().expect("incoming").remove(&id);
            self.incoming_meta
                .lock()
                .expect("incoming meta")
                .remove(&id);
            let msg = self.receiver.decide(id, FileOfferDecision::Reject)?;
            self.send_control(from, &msg).await
        })
    }

    fn pause(&self, id: TransferId) -> BoxFut<'_> {
        Box::pin(async move {
            let mut out = self.outgoing.lock().expect("outgoing");
            let o = out.get_mut(&id).ok_or(FilesError::UnknownTransfer)?;
            o.paused = true;
            if let Some(s) = &o.sender {
                s.on_receiver_message(&FileControlV1::Pause { transfer_id: id })?;
            }
            self.sched.lock().expect("sched").set_paused(id, true);
            Ok(())
        })
    }

    fn resume(&self, id: TransferId) -> BoxFut<'_> {
        Box::pin(async move {
            let mut out = self.outgoing.lock().expect("outgoing");
            let o = out.get_mut(&id).ok_or(FilesError::UnknownTransfer)?;
            o.paused = false;
            self.sched.lock().expect("sched").set_paused(id, false);
            if let Some(s) = &o.sender {
                s.on_receiver_message(&FileControlV1::Resume(ChunkRequestV1 {
                    transfer_id: id,
                    ranges: Vec::new(),
                }))?;
            }
            Ok(())
        })
    }

    fn cancel(&self, id: TransferId) -> BoxFut<'_> {
        Box::pin(async move {
            let to = {
                let out = self.outgoing.lock().expect("outgoing");
                let o = out.get(&id).ok_or(FilesError::UnknownTransfer)?;
                if let Some(s) = &o.sender {
                    s.on_receiver_message(&FileControlV1::Cancel(CancelV1 {
                        transfer_id: id,
                        keep_partial: false,
                        reason: None,
                    }))?;
                }
                o.to
            };
            self.drop_outgoing(id);
            self.send_control(
                to,
                &FileControlV1::Cancel(CancelV1 {
                    transfer_id: id,
                    keep_partial: false,
                    reason: None,
                }),
            )
            .await
        })
    }
}
