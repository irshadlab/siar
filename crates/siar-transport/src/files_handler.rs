//! `files/1` over iroh (spec 05 §207 Phase 6): a frame transport for the
//! file subsystem. The transport carries opaque, length-prefixed frames
//! with a one-byte kind tag; what they mean is `siar-files`' business
//! (this crate sits below it and must not depend on it, plan.md §86).
//!
//! **Ordering.** `files/1` assumes frames from one peer arrive in the
//! order they were sent (capabilities before the offer, the manifest
//! request before the chunk request). Separate QUIC streams are not
//! ordered relative to each other, so the sender keeps **one persistent
//! unidirectional stream per peer** and writes frames to it back to back
//! under a lock; QUIC orders bytes within a stream. A failed stream is
//! dropped and replaced once; frames in flight on it are lost, which the
//! protocol already tolerates (the receiver re-requests what is missing).
//!
//! **Bounds.** Every inbound frame's length is checked against
//! `max_frame` *before* any buffer is reserved, and the allocation uses
//! `try_reserve_exact`; an oversized or malformed header ends that
//! stream, nothing more.
//!
//! Frame layout on the stream: `[kind: u8][len: u32 BE][len bytes]`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    EndpointId,
};

pub const FILES_ALPN: &[u8] = b"messenger/files/1";

/// Largest frame this transport will carry in either direction: a
/// default-limit manifest (64 MiB) plus slack; chunk frames are far
/// smaller and control frames smaller still.
pub const FILES_MAX_FRAME: usize = 80 * 1024 * 1024;

pub(crate) const FRAME_HEADER_LEN: usize = 5;

/// Receives every frame that arrives for `files/1`.
pub trait FilesFrameSink: Send + Sync + 'static {
    fn on_frame(
        &self,
        from: EndpointId,
        kind: u8,
        frame: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// A sink that can be handed to `SiarEndpoint::bind_with_files` before
/// the real consumer exists. (The endpoint has to be bound first, and the
/// file service needs the endpoint to send, so the two cannot both be
/// constructed "first".) Frames that arrive before [`Self::set`] are
/// dropped; the protocol retries.
#[derive(Default)]
pub struct DeferredFilesSink {
    inner: OnceLock<Arc<dyn FilesFrameSink>>,
}

impl DeferredFilesSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Returns `false` if a sink was already set.
    pub fn set(&self, sink: Arc<dyn FilesFrameSink>) -> bool {
        self.inner.set(sink).is_ok()
    }
}

impl FilesFrameSink for DeferredFilesSink {
    fn on_frame(
        &self,
        from: EndpointId,
        kind: u8,
        frame: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        match self.inner.get() {
            Some(s) => s.on_frame(from, kind, frame),
            None => Box::pin(async {}),
        }
    }
}

#[derive(Clone)]
pub struct FilesProtocolHandler {
    sink: Arc<dyn FilesFrameSink>,
    max_frame: usize,
}

impl std::fmt::Debug for FilesProtocolHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesProtocolHandler")
            .finish_non_exhaustive()
    }
}

impl FilesProtocolHandler {
    pub fn new(sink: Arc<dyn FilesFrameSink>) -> Self {
        Self::with_max_frame(sink, FILES_MAX_FRAME)
    }

    pub fn with_max_frame(sink: Arc<dyn FilesFrameSink>, max_frame: usize) -> Self {
        Self { sink, max_frame }
    }
}

/// Encodes a frame header.
pub(crate) fn header(kind: u8, len: u32) -> [u8; FRAME_HEADER_LEN] {
    let mut h = [0u8; FRAME_HEADER_LEN];
    h[0] = kind;
    h[1..].copy_from_slice(&len.to_be_bytes());
    h
}

/// Decodes a frame header into `(kind, len)`.
pub(crate) fn parse_header(h: &[u8; FRAME_HEADER_LEN]) -> (u8, u32) {
    (h[0], u32::from_be_bytes([h[1], h[2], h[3], h[4]]))
}

async fn read_frames(
    mut recv: iroh::endpoint::RecvStream,
    from: EndpointId,
    sink: Arc<dyn FilesFrameSink>,
    max_frame: usize,
) {
    loop {
        let mut h = [0u8; FRAME_HEADER_LEN];
        if recv.read_exact(&mut h).await.is_err() {
            return; // the stream ended (or failed): normal
        }
        let (kind, len) = parse_header(&h);
        let len = len as usize;
        if len > max_frame {
            tracing::warn!(%from, len, max_frame, "files frame over the limit; closing its stream");
            return;
        }
        let mut frame = Vec::new();
        if frame.try_reserve_exact(len).is_err() {
            tracing::warn!(%from, len, "could not reserve a files frame; closing its stream");
            return;
        }
        frame.resize(len, 0);
        if recv.read_exact(&mut frame).await.is_err() {
            return;
        }
        sink.on_frame(from, kind, frame).await;
    }
}

impl ProtocolHandler for FilesProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let from = connection.remote_id();
        // One reader task per unidirectional stream, for as long as the
        // connection lives (a sender may replace a failed stream).
        while let Ok(recv) = connection.accept_uni().await {
            tokio::spawn(read_frames(
                recv,
                from,
                Arc::clone(&self.sink),
                self.max_frame,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip() {
        for (k, l) in [(1u8, 0u32), (3, 1), (2, u32::MAX), (255, 12345)] {
            assert_eq!(parse_header(&header(k, l)), (k, l));
        }
    }

    #[test]
    fn the_largest_frame_fits_the_length_field() {
        assert!(u32::try_from(FILES_MAX_FRAME).is_ok());
    }

    #[tokio::test]
    async fn a_deferred_sink_drops_until_set_then_forwards() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Count(Arc<AtomicUsize>);
        impl FilesFrameSink for Count {
            fn on_frame(
                &self,
                _: EndpointId,
                _: u8,
                _: Vec<u8>,
            ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Box::pin(async {})
            }
        }
        let id = iroh::SecretKey::generate().public();
        let n = Arc::new(AtomicUsize::new(0));
        let d = DeferredFilesSink::new();
        d.on_frame(id, 1, vec![]).await;
        assert_eq!(n.load(Ordering::Relaxed), 0);
        assert!(d.set(Arc::new(Count(n.clone()))));
        d.on_frame(id, 1, vec![]).await;
        assert_eq!(n.load(Ordering::Relaxed), 1);
        assert!(!d.set(Arc::new(Count(n.clone()))), "set once");
    }
}
