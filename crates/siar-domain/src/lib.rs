#![forbid(unsafe_code)]

//! siar-domain: platform-independent core types.
//!
//! Rule (plan.md §86): this crate must never depend on Dioxus, stoolap,
//! iroh, or any platform API. Everything here is pure data + invariants,
//! so it is usable from the CLI, the desktop app, tests, and fuzz targets
//! without pulling in networking or storage.

mod attachment;
mod call;
mod connectivity;
mod delivery;
mod device;
mod error;
mod group;
mod ids;
mod lifecycle;
mod mailbox;
mod message;
mod priority;
mod retry;

pub use attachment::{
    AttachmentReference, BlobSize, BlobSizeError, MediaType, MAX_ATTACHMENT_BYTES,
};
pub use call::{
    adapt_quality, CallControlEvent, CallParticipant, CallState, MediaQuality, NetworkConditions,
};
pub use connectivity::{ConnectivityState, EffectiveConnectivity, TransportLink};
pub use delivery::DeliveryState;
pub use device::SyncCursor;
pub use error::DomainError;
pub use group::{
    fanout_targets, DurableGroupEvent, EphemeralGroupEvent, GroupEpoch, GroupMember, GroupState,
    MemberRole,
};
pub use ids::{AccountId, ConversationId, DeviceId, MessageId};
pub use lifecycle::{LifecycleState, LifecycleTransition};
pub use mailbox::{
    now_millis, prepare_envelope, MailboxCapability, MailboxEnvelope, MailboxError,
    MAX_MAILBOX_ENVELOPE_BYTES,
};
pub use message::{MessageContent, MessageText, TextParseError};
pub use priority::MessagePriority;
pub use retry::{backoff_millis, with_jitter};
