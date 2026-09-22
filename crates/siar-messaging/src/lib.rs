//! siar-messaging: ties storage + crypto + transport together behind one
//! `MessageService` (plan.md §54–55, §111–112's send/receive flows).
//!
//! Same caveat as siar-transport/siar-storage: this depends on both, so it
//! inherits their "needs your local rustc, not this sandbox's" status.
//!
//! Phase-1 scope note: full contact discovery (plan.md §41–42, QR/DHT/
//! username service) doesn't exist yet. `PeerTicket` is the Phase-1
//! stand-in — copy-paste a printed ticket to add a peer — explicitly not
//! meant to survive past Phase 1.

mod blob_bridge;
pub mod events;
mod group_service;
mod key_package_directory;
pub mod projections;
mod service;
pub mod stoolap_projections;
mod ticket;

pub use blob_bridge::StorageBlobStore;
pub use events::{
    conversation_stream_id, decode_messaging_event, MessagingEvent, EVENT_TYPE_MESSAGE_CREATED,
    EVENT_TYPE_MESSAGE_DELETED, EVENT_TYPE_MESSAGE_DELIVERED, EVENT_TYPE_MESSAGE_EDITED,
    EVENT_TYPE_MESSAGE_QUEUED, EVENT_TYPE_MESSAGE_READ, EVENT_TYPE_MESSAGE_RECEIVED,
    EVENT_TYPE_REACTION_ADDED, EVENT_TYPE_REACTION_REMOVED,
};
pub use projections::{
    ConversationSummary, ConversationSummaryProjection, ConversationSummaryQuery,
};
pub use service::{IncomingEvent, MessageService};
pub use stoolap_projections::StoolapConversationSummaryProjection;
pub use ticket::PeerTicket;
// `InMemoryDeviceDirectory` added to this re-export list — a real,
// pre-existing gap found while wiring the desktop group UI: the type
// is `pub` inside group_service.rs and `apps/cli`'s bootstrap already
// imports `siar_messaging::InMemoryDeviceDirectory` (main.rs's own
// `use` block), but it was never added to this crate's public
// re-exports, so that import could only ever have resolved if
// something else in the crate re-exported it under a glob — nothing
// does. This was latent until something (this desktop UI work) forced
// an actual compile of the import path.
pub use group_service::{
    DeviceDirectory, GroupService, GroupServiceError, InMemoryDeviceDirectory, MemberDevice,
};
pub use key_package_directory::{InMemoryKeyPackageDirectory, KeyPackageDirectory};
