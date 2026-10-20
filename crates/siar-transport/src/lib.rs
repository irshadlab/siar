//! siar-transport: the iroh wrapper (plan.md §9–10).
//!
//! ============================ NOT YET VERIFIED ============================
//! Written against iroh 0.95.1's actual source (`Endpoint::bind`,
//! `Endpoint::connect(addr, alpn)`, `Router::builder(endpoint).accept(alpn,
//! handler).spawn()`, `ProtocolHandler::accept`, `SecretKey::generate`) —
//! read directly out of the fetched crate source in this sandbox, not
//! guessed. What could *not* be confirmed here is that it actually
//! compiles: iroh 0.95.1 itself needs rustc >= 1.91 transitively (hickory,
//! netwatch, time, tokio-websockets), which this sandbox's apt-installable
//! ceiling (1.85.1) can't reach. Run `cargo test -p siar-transport` on
//! your machine to confirm.
//!
//! If your local build hits the same broken pre-release pin this sandbox
//! hit (`ed25519-dalek = "=3.0.0-pre.1"` inside `iroh-base`/`iroh`, and
//! `curve25519-dalek = "=5.0.0-pre.1"` inside `crypto_box`/`iroh-base`,
//! both incompatible with the currently-published `pkcs8 0.11.0`): vendor
//! those three crates locally, relax the exact pins to the final
//! `ed25519-dalek = "3.0.0"` / `curve25519-dalek = "5.0.0"` releases, and
//! add a `[patch.crates-io]` pointing at the vendored copies. That's a
//! packaging bug in iroh's current crates.io release, not in this code.
//! ============================================================================
//!
//! Rule (plan.md §9): the rest of the app never touches `iroh::Endpoint`
//! directly — it goes through `PeerTransport` (messaging) or
//! `SiarEndpoint::fetch_blob` (attachments).
//!
//! `blob_handler.rs` is a hand-rolled blob-transfer protocol, not
//! `iroh-blobs` — the latest published `iroh-blobs` (0.95.0) still
//! depends on `iroh = "0.93"`, confirmed directly against its
//! `Cargo.toml`, which is incompatible with the `iroh = "0.95.1"` the
//! rest of this workspace is built on. Swap it back in once `iroh-blobs`
//! ships a 0.95.x-compatible release.

mod blob_handler;
mod endpoint;
mod error;
mod files_handler;
mod handler;
mod local_discovery;
mod pool;

pub use blob_handler::{BlobStore, BLOB_ALPN};
pub use endpoint::{SiarEndpoint, MESSENGER_ALPN};
pub use error::TransportError;
pub use files_handler::{
    DeferredFilesSink, FilesFrameSink, FilesProtocolHandler, FILES_ALPN, FILES_MAX_FRAME,
};
pub use handler::{IncomingFrame, MessagingProtocolHandler};
pub use local_discovery::LocalPeerDirectory;

use siar_protocol::WireMessage;

/// Plan.md §9's `PeerTransport` abstraction. Application code depends on
/// this trait, never on `iroh::Endpoint` — that's what keeps `iroh` calls
/// out of `siar-messaging` and everywhere above it.
///
/// Takes a full `EndpointAddr` (not just an `EndpointId`) so a caller that
/// already knows the peer's direct addresses (e.g. from a `PeerTicket`)
/// can hand them over and skip depending on discovery infrastructure —
/// discarding known addrs down to a bare ID would force every send
/// through discovery even when it's unnecessary.
#[async_trait::async_trait]
pub trait PeerTransport: Send + Sync {
    async fn send(
        &self,
        peer: iroh::EndpointAddr,
        message: &WireMessage,
    ) -> Result<(), TransportError>;
}
