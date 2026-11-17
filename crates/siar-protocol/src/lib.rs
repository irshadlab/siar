#![forbid(unsafe_code)]

//! siar-protocol: the wire format.
//!
//! Rule (plan.md §60): the wire protocol and the internal domain model are
//! two different things. `domain::MessageContent` never gets serialized
//! directly onto the network — it is converted into a versioned
//! `protocol::v1` type first, so `domain` can evolve freely and old
//! clients speaking `v1` keep working when `v2` ships.

mod blob;
mod codec;
mod limits;
pub mod link;
mod mailbox;
mod mesh;
mod route_advertisement;
pub mod v1;

pub use blob::{BlobRequest, BlobResponse, MAX_BLOB_FRAME_BYTES};
pub use codec::{
    decode_frame, decode_frame_generic, encode_frame, encode_frame_generic, CodecError,
};
pub use limits::{MAX_CONTROL_FRAME_BYTES, MAX_TEXT_FRAME_BYTES};
pub use mailbox::{
    AnonymousMailboxCheckIn, DeviceKeyDirectory, MailboxCheckIn, MailboxCheckInError,
    TokenMailboxEnvelope, TokenMailboxStore,
};
pub use mesh::MeshEnvelope;
pub use route_advertisement::RouteAdvertisement;

use serde::{Deserialize, Serialize};

/// Outer wire envelope. Every frame that goes on the QUIC stream is one of
/// these, `version`-tagged so the receiver knows how to decode `payload`
/// before it even looks at message content (plan.md §11, §60).
///
/// `Mesh` is next.md §29's addition (Phase 7's discovered gap, fixed in
/// the next pass): a relay can match on this variant and route by
/// `MeshEnvelope::destination` alone, without ever attempting the
/// session-decryption `V1` requires — see `mesh.rs`'s doc comment for
/// the full rationale.
///
/// `MailboxCheckIn` is next.md §76–77's addition — see `mailbox.rs`'s
/// doc comment for why this needed a genuinely new message rather than
/// a tweak to `Mesh`: a relay offering bundles to *anyone it happens to
/// talk to* (this workspace's existing naive-forward behavior) is a
/// different, privacy-cheaper thing than a relay answering "what do you
/// have for me specifically," which is what a device explicitly
/// identifying itself is asking.
///
/// `TokenMailboxDeposit`/`AnonymousMailboxCheckIn` (a later pass) are
/// the unlinkable counterparts to `Mesh`/`MailboxCheckIn` — see
/// `mailbox.rs`'s `TokenMailboxEnvelope`/`AnonymousMailboxCheckIn` doc
/// comments for why they're separate wire shapes rather than a
/// `destination` enum variant grafted onto the existing two.
///
/// `RouteAdvertisement` (a later pass still) is a different kind of
/// message entirely — not something addressed to a destination at all,
/// but a relay telling a directly-connected peer about a route it
/// knows. See `route_advertisement.rs`'s doc comment for the full
/// picture, including its deliberately-unauthenticated trust model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMessage {
    V1(v1::Envelope),
    Mesh(MeshEnvelope),
    MailboxCheckIn(MailboxCheckIn),
    TokenMailboxDeposit(TokenMailboxEnvelope),
    AnonymousMailboxCheckIn(AnonymousMailboxCheckIn),
    RouteAdvertisement(RouteAdvertisement),
    /// Linking a new device to an account (see [`link`]).
    Link(link::LinkMessage),
}
