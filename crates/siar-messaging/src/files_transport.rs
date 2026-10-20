//! Glue between the file subsystem (`siar-files`, which speaks
//! `DeviceId` and opaque frames) and the iroh transport
//! (`siar-transport`, which speaks `EndpointId` and `files/1` frames).
//!
//! * [`PeerBook`] — who is who: the device behind an endpoint and the
//!   address to reach a device. The host fills it as peers are paired
//!   (a [`crate::PeerTicket`] carries the address).
//! * [`TransportFileChannel`] — `siar_files::FileChannel` over
//!   `SiarEndpoint::send_files_frame` (ordered per peer).
//! * [`FilesInbound`] — the `FilesFrameSink` that hands arriving frames to
//!   `FilesService::handle_frame`, resolving the sender through the book.
//!   Frames from an endpoint that is not in the book are dropped: an
//!   unknown peer cannot start a transfer.
//!
//! Wiring order (the endpoint must exist before the service, and the
//! service before the sink can be filled): create a
//! [`siar_transport::DeferredFilesSink`], bind the endpoint with it via
//! `SiarEndpoint::bind_with_files`, build the `FilesService` over a
//! [`TransportFileChannel`], then `deferred.set(Arc::new(FilesInbound::new(…)))`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::service::{AttachmentFetchError, AttachmentFetcher};
use crate::PeerTicket;
use iroh::{EndpointAddr, EndpointId};
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::ReferenceStore;
use siar_blob_manifest::reference_store::{BlobRole, ObjectId};
use siar_domain::AttachmentReference;
use siar_domain::DeviceId;
use siar_files::{
    ChannelError, Destination, DestinationPolicy, FileChannel, FilesError, FilesService, FrameKind,
};
use siar_transport::{FilesFrameSink, SiarEndpoint};

#[derive(Default)]
pub struct PeerBook {
    inner: Mutex<BookInner>,
}

#[derive(Default)]
struct BookInner {
    addr_of: HashMap<DeviceId, EndpointAddr>,
    device_of: HashMap<EndpointId, DeviceId>,
    /// A device's X25519 public key, when known (from its `PeerTicket`):
    /// what the pairwise key-wrapping secret is derived against.
    x25519_of: HashMap<DeviceId, [u8; 32]>,
}

impl PeerBook {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records (or updates) a device's address. An endpoint maps to one
    /// device: re-registering it under a new device replaces the old
    /// mapping, so a stale pairing cannot keep receiving.
    pub fn register(&self, device: DeviceId, addr: EndpointAddr) {
        let mut b = self.inner.lock().expect("peer book");
        if let Some(old) = b.addr_of.insert(device, addr.clone()) {
            if old.id != addr.id {
                b.device_of.remove(&old.id);
            }
        }
        if let Some(prev_device) = b.device_of.insert(addr.id, device) {
            if prev_device != device {
                b.addr_of.remove(&prev_device);
            }
        }
    }

    /// [`Self::register`] plus the device's X25519 public key, enabling
    /// [`IdentityPairwiseSecrets`] for it.
    pub fn register_with_key(&self, device: DeviceId, addr: EndpointAddr, x25519_public: [u8; 32]) {
        self.register(device, addr);
        self.inner
            .lock()
            .expect("peer book")
            .x25519_of
            .insert(device, x25519_public);
    }

    pub fn x25519_of(&self, device: DeviceId) -> Option<[u8; 32]> {
        self.inner
            .lock()
            .expect("peer book")
            .x25519_of
            .get(&device)
            .copied()
    }

    pub fn forget(&self, device: DeviceId) {
        let mut b = self.inner.lock().expect("peer book");
        b.x25519_of.remove(&device);
        if let Some(addr) = b.addr_of.remove(&device) {
            b.device_of.remove(&addr.id);
        }
    }

    pub fn addr_of(&self, device: DeviceId) -> Option<EndpointAddr> {
        self.inner
            .lock()
            .expect("peer book")
            .addr_of
            .get(&device)
            .cloned()
    }

    pub fn device_of(&self, endpoint: EndpointId) -> Option<DeviceId> {
        self.inner
            .lock()
            .expect("peer book")
            .device_of
            .get(&endpoint)
            .copied()
    }
}

/// Pairwise key-wrapping secrets from this device's real identity and each
/// peer's X25519 public key (`siar_crypto::FileKeyWrapSecret`). A peer with
/// no recorded key has no secret, so nothing is ever wrapped for it with a
/// guess.
pub struct IdentityPairwiseSecrets {
    identity: Arc<siar_crypto::DeviceIdentity>,
    book: Arc<PeerBook>,
}

impl IdentityPairwiseSecrets {
    pub fn new(identity: Arc<siar_crypto::DeviceIdentity>, book: Arc<PeerBook>) -> Self {
        Self { identity, book }
    }
}

impl siar_files::PairwiseSecrets for IdentityPairwiseSecrets {
    fn secret_for(&self, peer: DeviceId) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        let public = x25519_dalek::PublicKey::from(self.book.x25519_of(peer)?);
        let secret = siar_crypto::FileKeyWrapSecret::establish(&self.identity, &public);
        Some(zeroize::Zeroizing::new(*secret.as_bytes()))
    }
}

/// `FileChannel` over the iroh endpoint.
pub struct TransportFileChannel {
    endpoint: Arc<SiarEndpoint>,
    book: Arc<PeerBook>,
}

impl TransportFileChannel {
    pub fn new(endpoint: Arc<SiarEndpoint>, book: Arc<PeerBook>) -> Self {
        Self { endpoint, book }
    }
}

impl FileChannel for TransportFileChannel {
    async fn send_frame(
        &self,
        to: DeviceId,
        kind: FrameKind,
        frame: Vec<u8>,
    ) -> Result<(), ChannelError> {
        let addr = self.book.addr_of(to).ok_or(ChannelError::Unreachable)?;
        self.endpoint
            .send_files_frame(addr, kind as u8, &frame)
            .await
            .map_err(|e| {
                tracing::debug!(error = %e, "files frame not sent");
                ChannelError::Unreachable
            })
    }
}

/// The receiving half: arriving frames → `FilesService::handle_frame`.
pub struct FilesInbound<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    service: Arc<FilesService<C, M, R>>,
    book: Arc<PeerBook>,
}

impl<C, M, R> FilesInbound<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    pub fn new(service: Arc<FilesService<C, M, R>>, book: Arc<PeerBook>) -> Self {
        Self { service, book }
    }
}

impl<C, M, R> FilesFrameSink for FilesInbound<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    fn on_frame(
        &self,
        from: EndpointId,
        kind: u8,
        frame: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let Some(device) = self.book.device_of(from) else {
                tracing::debug!(%from, "files frame from an unpaired endpoint dropped");
                return;
            };
            if let Err(e) = self.service.handle_frame(device, kind, &frame).await {
                tracing::debug!(error = %e, "files frame rejected");
            }
        })
    }
}

/// Pulls the attachment behind `reference` from the peer at `peer` over
/// `files/1` — chunked, resumable, hash-verified, and referenced under
/// `dest` — and returns the decrypted bytes. The chunked replacement for
/// `MessageService::fetch_attachment`'s whole-blob request, usable with
/// the same inputs (the reference from the message, and the sender's
/// `PeerTicket::endpoint_addr.id`).
///
/// Fails with [`FilesError::BlobUnavailable`] if the peer does not have
/// or will not serve it, [`FilesError::FetchTimeout`] if it does not
/// arrive in time, and an unreachable-channel error if `peer` is not in
/// the book (an unpaired endpoint is never dialed for files).
pub async fn fetch_attachment_over_files<C, M, R>(
    service: &Arc<FilesService<C, M, R>>,
    book: &PeerBook,
    peer: EndpointId,
    reference: &AttachmentReference,
    dest: DestinationPolicy,
) -> Result<Vec<u8>, FilesError>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    let device = book
        .device_of(peer)
        .ok_or(FilesError::Channel(ChannelError::Unreachable))?;
    service
        .fetch_attachment(Destination::Device(device), reference, dest)
        .await
}

/// [`AttachmentFetcher`] over `files/1`: what `MessageService` is given so
/// `fetch_attachment` pulls chunked and resumable, falling back to the
/// legacy path when the peer cannot speak `files/1`. Fetched blobs are
/// kept in the file store under `fetched-<hash prefix>`.
pub struct FilesAttachmentFetcher<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    service: Arc<FilesService<C, M, R>>,
    book: Arc<PeerBook>,
}

impl<C, M, R> FilesAttachmentFetcher<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    pub fn new(service: Arc<FilesService<C, M, R>>, book: Arc<PeerBook>) -> Self {
        Self { service, book }
    }
}

impl<C, M, R> AttachmentFetcher for FilesAttachmentFetcher<C, M, R>
where
    C: FileChannel + 'static,
    M: BlobMetadataStore + 'static,
    R: ReferenceStore + ?Sized + 'static,
{
    fn fetch<'a>(
        &'a self,
        peer: &'a PeerTicket,
        reference: &'a AttachmentReference,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Vec<u8>, AttachmentFetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let hex: String = reference
                .blob_hash
                .iter()
                .take(16)
                .map(|b| format!("{b:02x}"))
                .collect();
            let object = ObjectId::new(&format!("fetched-{hex}"))
                .map_err(|_| AttachmentFetchError::Failed)?;
            let dest = DestinationPolicy {
                object,
                role: BlobRole::MessageAttachment,
                expires: TransferExpiry(u64::MAX),
            };
            fetch_attachment_over_files(
                &self.service,
                &self.book,
                peer.endpoint_addr.id,
                reference,
                dest,
            )
            .await
            .map_err(|e| match e {
                FilesError::BlobUnavailable => AttachmentFetchError::Unavailable,
                FilesError::Channel(_) | FilesError::NoCapabilities(_) | FilesError::NoResponse => {
                    AttachmentFetchError::Unsupported
                }
                _ => AttachmentFetchError::Failed,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    #[test]
    fn the_book_maps_both_ways_and_forgets() {
        let book = PeerBook::new();
        let (d, a) = (DeviceId::new(), addr());
        book.register(d, a.clone());
        assert_eq!(book.addr_of(d).unwrap().id, a.id);
        assert_eq!(book.device_of(a.id), Some(d));
        book.forget(d);
        assert!(book.addr_of(d).is_none());
        assert!(book.device_of(a.id).is_none());
    }

    #[test]
    fn re_registering_an_endpoint_under_a_new_device_replaces_the_old_pairing() {
        let book = PeerBook::new();
        let a = addr();
        let (old, new) = (DeviceId::new(), DeviceId::new());
        book.register(old, a.clone());
        book.register(new, a.clone());
        assert_eq!(book.device_of(a.id), Some(new));
        assert!(
            book.addr_of(old).is_none(),
            "the stale device no longer resolves"
        );
    }

    #[test]
    fn a_device_that_moves_endpoints_drops_its_old_endpoint() {
        let book = PeerBook::new();
        let d = DeviceId::new();
        let (a1, a2) = (addr(), addr());
        book.register(d, a1.clone());
        book.register(d, a2.clone());
        assert!(book.device_of(a1.id).is_none());
        assert_eq!(book.device_of(a2.id), Some(d));
    }
}
