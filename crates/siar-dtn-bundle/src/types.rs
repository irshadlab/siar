//! §5 "Bundle Concept", §6 "Bundle Structure", §10 "Bundle
//! Destination", §11 "Routing Tokens", §12 "Bundle ID", §17 "Storage
//! Classes", §36 "DTN Priority".

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// §12: "must be stable across retries and replicas... Do not create a
/// new BundleId for every hop." A random UUID, generated once at
/// bundle creation — same reasoning `siar_event_log::EventId` already
/// documents for the same "stable, offline, collision-resistant"
/// requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct BundleId(Uuid);

impl BundleId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for BundleId {
    fn default() -> Self {
        Self::new()
    }
}

/// §11: "opaque, short-lived where possible, difficult to correlate
/// long-term... Permanent account IDs should not be advertised
/// unnecessarily in public BLE beacons or relay headers." Raw bytes,
/// not `siar_domain::AccountId`/`DeviceId` directly — the entire point
/// of this type is to NOT be a stable, correlatable identifier; a real
/// implementation would derive one (e.g. an HMAC of the real id under a
/// rotating key), which needs `siar-crypto`, not this crate (see this
/// crate's own top doc comment on scope).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteToken(pub Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BroadcastScope {
    pub radius_hops: u8,
}

/// §10, verbatim variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DtnDestination {
    DeviceOpaque(RouteToken),
    AccountOpaque(RouteToken),
    GroupOpaque(RouteToken),
    LocalBroadcast(BroadcastScope),
}

/// §6's own field — not detailed further in the spec text this crate
/// was built against beyond appearing as a bundle field. A route token,
/// same shape as [`DtnDestination`]'s opaque forms — the *sender's*
/// identity deserves the same non-correlation treatment §11 asks of the
/// destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DtnSource(pub RouteToken);

/// §6's field — an application-defined tag for what kind of payload
/// this bundle carries (a message, a file chunk, an event, ...),
/// analogous to `siar_event_log::EventTypeId`'s own "plain numeric tag,
/// caller assigns constants" choice, for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayloadTypeId(pub u32);

/// §17, verbatim variants: "Each can have different retention and
/// eviction policies" — the policies themselves aren't specified in
/// the source text beyond that statement, so this crate defines the
/// classification only; an actual differentiated eviction policy per
/// class is real, separate follow-up work (this crate's existing
/// sibling, `siar-dtn`, already has a working priority-based eviction
/// order — see this crate's own top doc comment for how the two
/// relate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageClass {
    LocalOrigin,
    Relay,
    CriticalEmergency,
    DeliveryReceipt,
}

/// §36's own field name (`DtnPriority`) — spec-defined priority tiers
/// specifically for DTN bundle store-carry-forward scheduling and
/// spray allocation (see §22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DtnPriority {
    Low,
    Normal,
    Important,
    Sos,
}

impl DtnPriority {
    /// §22's own worked examples: "normal message = 2, important
    /// message = 4, SOS = 8."
    pub fn default_replication_budget(self) -> u8 {
        match self {
            Self::Low => 1,
            Self::Normal => 2,
            Self::Important => 4,
            Self::Sos => 8,
        }
    }
}

/// §6's field — the spec names this type but doesn't enumerate its
/// variants in the text this crate was built against; `DirectOnly`/
/// `SprayAndWait`/`GatewayPreferred` are §189 Phase 4's own named
/// strategies, reused here as the natural variant set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardingClass {
    DirectOnly,
    SprayAndWait,
    GatewayPreferred,
}
