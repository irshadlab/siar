//! iroh `Endpoint` + `Router` wrapper (plan.md §9–10).

use crate::{
    blob_handler::{BlobProtocolHandler, BlobStore, BLOB_ALPN},
    files_handler::{FilesFrameSink, FilesProtocolHandler, FILES_ALPN, FILES_MAX_FRAME},
    handler::MessagingProtocolHandler,
    local_discovery::{spawn_local_discovery_task, LocalPeerDirectory},
    pool::ConnectionPool,
    PeerTransport, TransportError,
};
use iroh::{protocol::Router, Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use siar_protocol::{
    decode_frame_generic, encode_frame, encode_frame_generic, BlobRequest, BlobResponse,
    WireMessage, MAX_BLOB_FRAME_BYTES,
};
use std::sync::Arc;
use tokio::sync::mpsc;

/// plan.md §10's example protocol identifiers, ours for text messaging.
pub const MESSENGER_ALPN: &[u8] = b"messenger/msg/1";

/// One `iroh::Endpoint` per running application (plan.md §10), wrapped so
/// nothing above this crate imports `iroh` directly (plan.md §9).
pub struct SiarEndpoint {
    endpoint: Endpoint,
    _router: Router,
    pool: ConnectionPool,
    local_peers: Arc<LocalPeerDirectory>,
    /// One persistent, ordered `files/1` stream per peer (see
    /// `files_handler`'s module docs).
    files_streams: std::sync::Mutex<
        std::collections::HashMap<
            EndpointId,
            Arc<tokio::sync::Mutex<Option<iroh::endpoint::SendStream>>>,
        >,
    >,
}

impl SiarEndpoint {
    /// Binds a fresh endpoint under `secret_key`, routing `MESSENGER_ALPN`
    /// connections to `MessagingProtocolHandler` (forwarding decoded
    /// frames onto `incoming`) and `BLOB_ALPN` connections to a
    /// `BlobProtocolHandler` backed by `blob_store` (plan.md §22's
    /// attachment flow — see `blob_handler.rs`'s module docs for why
    /// this isn't `iroh-blobs`).
    pub async fn bind(
        secret_key: SecretKey,
        incoming: mpsc::Sender<crate::handler::IncomingFrame>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Result<Self, TransportError> {
        Self::bind_with_files(secret_key, incoming, blob_store, None).await
    }

    /// [`Self::bind`], additionally routing `FILES_ALPN` connections to
    /// `files` (spec 05 `files/1`). With `None` the endpoint does not
    /// accept that protocol, exactly as before.
    pub async fn bind_with_files(
        secret_key: SecretKey,
        incoming: mpsc::Sender<crate::handler::IncomingFrame>,
        blob_store: Arc<dyn BlobStore>,
        files: Option<Arc<dyn FilesFrameSink>>,
    ) -> Result<Self, TransportError> {
        // iroh 1.0's `Endpoint::builder` takes a `Preset` (a bundle of
        // defaults) instead of the old zero-arg builder. `presets::N0`
        // is the closest match to what 0.95.1's argument-less builder
        // did implicitly: n0's DNS-based discovery + default relay set
        // (plan.md §9-10's discovery/relay infra), gated behind the
        // `tls-ring` feature — which is one of iroh's own default
        // features, so no Cargo.toml change is needed for it.
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key)
            .bind()
            .await
            .map_err(|e| TransportError::Bind(e.to_string()))?;

        // next.md §12: local-network discovery, run alongside the
        // default DNS-based discovery `presets::N0` already configures
        // above — this is the piece that keeps two devices on the same
        // router/hotspot messaging when the Internet is down (next.md
        // §2's "Internet degraded... no Internet but Wi-Fi/router
        // available"). See `local_discovery.rs`'s module doc for why
        // this needs a separate crate rather than an `iroh` feature
        // flag, and for what it deliberately does and doesn't advertise.
        let local_peers = LocalPeerDirectory::new();
        let mdns = MdnsAddressLookup::builder()
            .build(endpoint.id())
            // Exact error type not independently confirmed beyond "the
            // crate's own example unwraps this Result" — Display via
            // `{e}` matches every other iroh-ecosystem error type seen
            // so far in this workspace, but flagging the assumption
            // rather than presenting it as verified.
            .map_err(|e| TransportError::Bind(format!("mdns local discovery: {e}")))?;
        endpoint
            .address_lookup()
            // Corrected against real `cargo build` output: this returns
            // `Result`, not `Option` — my original `.ok_or_else` was
            // wrong. `map_err` is the fix; exact error type still not
            // independently confirmed beyond "implements Display," same
            // flagged assumption as the `mdns.builder().build(...)` call
            // just above.
            .map_err(|e| {
                TransportError::Bind(format!(
                    "endpoint has no address_lookup registry under presets::N0: {e}"
                ))
            })?
            .add(mdns.clone());
        spawn_local_discovery_task(mdns, Arc::clone(&local_peers));

        let messaging_handler = MessagingProtocolHandler::new(incoming);
        let blob_handler = BlobProtocolHandler::new(blob_store);
        let mut builder = Router::builder(endpoint.clone())
            .accept(MESSENGER_ALPN, messaging_handler)
            .accept(BLOB_ALPN, blob_handler);
        if let Some(sink) = files {
            builder = builder.accept(FILES_ALPN, FilesProtocolHandler::new(sink));
        }
        let router = builder.spawn();

        Ok(Self {
            endpoint,
            _router: router,
            pool: ConnectionPool::default(),
            local_peers,
            files_streams: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// next.md §60's UI network status: addresses of peers mDNS has
    /// found on the local network right now. Empty means either no
    /// local peers are running siar, or none are reachable on this LAN
    /// — this method doesn't distinguish those, same as the doc's own
    /// "Internet unavailable / 3 nearby relay devices" example doesn't
    /// need to.
    pub fn local_peers(&self) -> Vec<EndpointAddr> {
        self.local_peers.snapshot()
    }

    /// plan.md §35: called periodically by the retry scheduler to drop
    /// dead connections out of the pool rather than holding them forever.
    pub fn evict_idle_connections(&self) {
        self.pool.evict_closed();
    }

    async fn connection_for(
        &self,
        peer: EndpointAddr,
        alpn: &[u8],
    ) -> Result<iroh::endpoint::Connection, TransportError> {
        if let Some(conn) = self.pool.get_live(&peer.id, alpn) {
            return Ok(conn);
        }
        let connection = self
            .endpoint
            .connect(peer.clone(), alpn)
            .await
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        self.pool.insert(peer.id, alpn, connection.clone());
        Ok(connection)
    }

    /// Sends one `files/1` frame to `peer`, in order after every frame
    /// previously sent to that peer. The stream is opened on first use and
    /// reused; if a write fails it is replaced once and the frame retried
    /// on the new stream. Frames over [`FILES_MAX_FRAME`] are refused
    /// locally.
    pub async fn send_files_frame(
        &self,
        peer: EndpointAddr,
        kind: u8,
        frame: &[u8],
    ) -> Result<(), TransportError> {
        if frame.len() > FILES_MAX_FRAME {
            return Err(TransportError::Write("files frame over the limit".into()));
        }
        let len = u32::try_from(frame.len())
            .map_err(|_| TransportError::Write("files frame over the limit".into()))?;
        let hdr = crate::files_handler::header(kind, len);
        let slot = {
            let mut map = self.files_streams.lock().expect("files streams");
            Arc::clone(map.entry(peer.id).or_default())
        };
        // Holding this lock across the write is what keeps frames ordered.
        let mut guard = slot.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                let connection = self.connection_for(peer.clone(), FILES_ALPN).await?;
                let stream = connection
                    .open_uni()
                    .await
                    .map_err(|e| TransportError::Write(e.to_string()))?;
                *guard = Some(stream);
            }
            let stream = guard.as_mut().expect("just ensured");
            let written = async {
                stream.write_all(&hdr).await?;
                stream.write_all(frame).await
            }
            .await;
            match written {
                Ok(()) => return Ok(()),
                Err(e) if attempt == 1 => return Err(TransportError::Write(e.to_string())),
                Err(_) => *guard = None, // replace the stream and retry once
            }
        }
        unreachable!("the loop returns on its second iteration")
    }

    /// plan.md §22's attachment flow: ask `peer` for the blob addressed
    /// by `blob_hash`. `Ok(None)` means the peer answered "I don't have
    /// it" (`BlobResponse::NotFound`), distinct from a transport error.
    pub async fn fetch_blob(
        &self,
        peer: EndpointAddr,
        blob_hash: [u8; 32],
    ) -> Result<Option<Vec<u8>>, TransportError> {
        let connection = self.connection_for(peer, BLOB_ALPN).await?;
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| TransportError::Write(e.to_string()))?;

        let mut framed = Vec::new();
        encode_frame_generic(
            &BlobRequest { blob_hash },
            MAX_BLOB_FRAME_BYTES,
            &mut framed,
        )?;
        send.write_all(&framed)
            .await
            .map_err(|e| TransportError::Write(e.to_string()))?;
        send.finish()
            .map_err(|e| TransportError::Write(e.to_string()))?;

        let bytes = recv
            .read_to_end(MAX_BLOB_FRAME_BYTES)
            .await
            .map_err(|e| TransportError::Read(e.to_string()))?;
        let (response, _consumed): (BlobResponse, usize) =
            decode_frame_generic(&bytes, MAX_BLOB_FRAME_BYTES)?;

        match response {
            BlobResponse::Found { ciphertext } => Ok(Some(ciphertext)),
            BlobResponse::NotFound => Ok(None),
        }
    }
}

#[async_trait::async_trait]
impl PeerTransport for SiarEndpoint {
    async fn send(&self, peer: EndpointAddr, message: &WireMessage) -> Result<(), TransportError> {
        // plan.md §34: reuse a pooled connection where one is live,
        // rather than dialing fresh for every message.
        let connection = self.connection_for(peer, MESSENGER_ALPN).await?;

        let (mut send, _recv) = connection
            .open_bi()
            .await
            .map_err(|e| TransportError::Write(e.to_string()))?;

        let mut framed = Vec::new();
        encode_frame(message, &mut framed)?;

        send.write_all(&framed)
            .await
            .map_err(|e| TransportError::Write(e.to_string()))?;
        send.finish()
            .map_err(|e| TransportError::Write(e.to_string()))?;

        Ok(())
    }
}
