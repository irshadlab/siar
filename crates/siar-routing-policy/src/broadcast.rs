//! §174 "Broadcast Routing".
//!
//! "Emergency/local broadcast may use: local dissemination, mesh,
//! DTN. Do not reuse ordinary unicast blindly." The transport
//! restriction is exactly [`crate::scope::RouteScope::LocalOnly`]
//! (§134, already real): its own list — LAN, Wi-Fi Direct/Aware,
//! Bluetooth, mesh, DTN — is the spec's "local dissemination, mesh,
//! DTN" read at the transport-kind level; no separate broadcast-
//! specific scope was needed. [`crate::scope::eliminate_out_of_scope_candidates`]
//! applied with that scope *is* "do not reuse ordinary unicast
//! blindly" — `IrohDirect`/`IrohRelay` never pass it.
//!
//! "With separate duplication controls" is the one genuinely new
//! piece: [`BroadcastDeliveryTracker`]. A caller fanning a broadcast
//! out to several devices needs to know which ones it's already
//! reached so a retry or an overlapping mesh path doesn't redeliver
//! the same broadcast to the same device twice — this crate has no
//! delivery-confirmation channel of its own (a caller's
//! [`crate::engine::RouteResultReport`] is what would actually mark
//! one), so the tracker is deliberately just a bounded-by-usage set a
//! caller drives, the same "explicit, caller-owned state" shape
//! [`crate::diagnostics::RouteHistory`] already takes for a different
//! kind of history.

use std::collections::HashSet;

use siar_domain::DeviceId;

/// A broadcast's own identity — distinct from
/// [`crate::descriptor::OperationId`], since one broadcast operation
/// may need to track delivery per *device*, and an `OperationId`
/// alone says nothing about which devices have already received it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BroadcastId(pub uuid::Uuid);

impl BroadcastId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for BroadcastId {
    fn default() -> Self {
        Self::new()
    }
}

/// §174's own "separate duplication controls." Tracks, per broadcast,
/// which devices have already been marked delivered — nothing more;
/// eviction of old broadcasts entirely is a caller's call (this
/// crate's usual "bounded, but the caller decides when to reset,"
/// same as [`crate::risk::PathPenalty`] leaving expiry timing to the
/// caller rather than this type guessing a lifetime for it).
#[derive(Debug, Clone, Default)]
pub struct BroadcastDeliveryTracker {
    delivered: HashSet<(BroadcastId, DeviceId)>,
}

impl BroadcastDeliveryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_delivered(&mut self, broadcast: BroadcastId, device: DeviceId) {
        self.delivered.insert((broadcast, device));
    }

    pub fn already_delivered(&self, broadcast: BroadcastId, device: DeviceId) -> bool {
        self.delivered.contains(&(broadcast, device))
    }

    /// Drops every recorded delivery for one broadcast — a caller's
    /// signal that the broadcast itself is finished (delivered,
    /// expired, or cancelled) and its bookkeeping can be released,
    /// rather than this tracker growing forever across unrelated
    /// broadcasts.
    pub fn clear_broadcast(&mut self, broadcast: BroadcastId) {
        self.delivered.retain(|(id, _)| *id != broadcast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{PathCandidate, TransportEndpoint};
    use crate::metrics::PathMetrics;
    use crate::scope::{eliminate_out_of_scope_candidates, RouteScope};
    use crate::types::{
        MeteredState, PathCapabilities, PathId, RoamingState, RouteHealth, TransportKind,
    };

    fn candidate(transport: TransportKind) -> PathCandidate {
        PathCandidate {
            path_id: PathId::new(),
            transport,
            peer: DeviceId::new(),
            endpoint: TransportEndpoint(Vec::new()),
            metrics: PathMetrics::unknown(),
            capabilities: PathCapabilities {
                reliable_stream: true,
                datagram: false,
                large_files: false,
                realtime_media: false,
                peer_discovery: false,
                store_and_forward: false,
                metered: MeteredState::Unknown,
                roaming: RoamingState::Unknown,
                requires_foreground: false,
            },
            health: RouteHealth::Healthy,
            underlay: None,
            state: crate::acquisition::CandidateState::Active,
        }
    }

    #[test]
    fn spec_174_broadcast_never_falls_back_to_ordinary_internet_unicast() {
        let candidates = vec![
            candidate(TransportKind::IrohDirect),
            candidate(TransportKind::Dtn),
        ];
        let kept = eliminate_out_of_scope_candidates(&candidates, RouteScope::LocalOnly);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].transport, TransportKind::Dtn);
    }

    #[test]
    fn a_device_marked_delivered_is_not_redelivered_to() {
        let mut tracker = BroadcastDeliveryTracker::new();
        let broadcast = BroadcastId::new();
        let device = DeviceId::new();

        assert!(!tracker.already_delivered(broadcast, device));
        tracker.mark_delivered(broadcast, device);
        assert!(tracker.already_delivered(broadcast, device));
    }

    #[test]
    fn clearing_a_broadcast_only_drops_its_own_records() {
        let mut tracker = BroadcastDeliveryTracker::new();
        let a = BroadcastId::new();
        let b = BroadcastId::new();
        let device = DeviceId::new();
        tracker.mark_delivered(a, device);
        tracker.mark_delivered(b, device);

        tracker.clear_broadcast(a);
        assert!(!tracker.already_delivered(a, device));
        assert!(tracker.already_delivered(b, device));
    }
}
