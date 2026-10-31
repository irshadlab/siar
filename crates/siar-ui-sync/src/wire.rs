//! Wire types. **Domain data, not UI structs.**
//!
//! Each [`Section`] is a self-contained, idempotent replacement of one slice
//! of daemon-owned state. Applying the same section twice, or a newer one
//! over an older one, is always safe — which is what lets snapshots and
//! deltas overlap without a coordination protocol.
//!
//! State that belongs to the *window* (selection, composer text, filter,
//! banner dismissals, scroll) is deliberately absent: the daemon must never
//! know it, and a second window must not inherit it.

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, ConversationId, DeliveryState, DeviceId, MessageContent, MessageId};
use siar_runtime::Topic;

/// The event-bus topic sync frames travel on.
pub const SYNC_TOPIC: Topic = Topic(0x100);
/// One section's encoded size ceiling (a timeline beyond it is truncated at
/// the old end and says so via `has_more`).
pub const MAX_SECTION_BYTES: usize = 192 * 1024;
/// Target upper bound for one frame's payload; frames are split to fit, well
/// inside the IPC layer's 1 MiB frame limit.
pub const MAX_FRAME_PAYLOAD: usize = 384 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SectionKey {
    Core,
    Identity,
    Conversations,
    Contacts,
    Network,
    Power,
    Timeline(ConversationId),
}

impl SectionKey {
    /// Deterministic order for frames and tests.
    #[must_use]
    pub fn sort_key(&self) -> (u8, u128) {
        match self {
            Self::Core => (0, 0),
            Self::Identity => (1, 0),
            Self::Conversations => (2, 0),
            Self::Contacts => (3, 0),
            Self::Network => (4, 0),
            Self::Power => (5, 0),
            Self::Timeline(id) => (6, id.as_uuid().as_u128()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreDto {
    pub ready: bool,
    pub startup_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityDto {
    pub account: Option<AccountId>,
    pub device: Option<DeviceId>,
    pub ticket: String,
    pub key_package: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDto {
    pub id: ConversationId,
    pub display_name: String,
    pub preview: Option<String>,
    pub unread: u32,
    pub group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactDto {
    pub device_id: DeviceId,
    pub account_id: AccountId,
    pub display_name: String,
    pub ticket_text: String,
    pub key_package_b64: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyDto {
    Unverified,
    Verified,
    KeyChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyDto {
    pub number: String,
    pub state: VerifyDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsDto {
    pub contacts: Vec<ContactDto>,
    pub safety: Vec<(DeviceId, SafetyDto)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetDto {
    Online,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReachDto {
    Connected,
    Connecting,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDto {
    pub overall: Option<NetDto>,
    pub peers: Vec<(DeviceId, ReachDto)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerDto {
    pub headline: String,
    pub details: Vec<String>,
    pub waiting: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryDto {
    pub message_id: MessageId,
    pub sequence: u64,
    pub content: MessageContent,
    pub delivery: DeliveryState,
    pub from_me: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PagingDto {
    pub loaded: bool,
    pub next_before: Option<u64>,
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineDto {
    /// Oldest first.
    pub entries: Vec<EntryDto>,
    pub has_more: bool,
    pub paging: PagingDto,
    pub sendable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    Core(CoreDto),
    Identity(IdentityDto),
    Conversations(Vec<ConversationDto>),
    Contacts(ContactsDto),
    Network(NetworkDto),
    Power(Option<PowerDto>),
    Timeline(TimelineDto),
}

/// One section's change. `section: None` means the section is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionUpdate {
    pub key: SectionKey,
    /// Per-section counter, bumped whenever its content changes.
    pub version: u64,
    pub section: Option<Section>,
}

/// A batch of updates.
///
/// * `full` frames replace **all** synced state (the first frame of a
///   snapshot); non-`full` frames are deltas.
/// * `seq` is per daemon instance and gap-free: a client that sees a gap
///   (or a different `instance_id`) must resync from a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncFrame {
    pub instance_id: u128,
    pub seq: u64,
    pub full: bool,
    pub updates: Vec<SectionUpdate>,
}

pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_allocvec(v).unwrap_or_default()
}

/// Bounded decode: the caller has already limited `bytes` to a frame.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}
