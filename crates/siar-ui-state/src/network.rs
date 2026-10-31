//! `NetworkState` (plan.md §53). Deliberately coarse — a settings/status
//! badge, not a diagnostics panel; per-peer reachability lives alongside
//! it for the "connecting to Bob..." indicator without pretending the
//! app knows a general answer to "am I online" beyond that.

use siar_domain::DeviceId;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerReachability {
    Connected,
    Connecting,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Online,
    Offline,
}

#[derive(Debug, Default)]
pub struct NetworkStatus {
    overall: Option<NetworkState>,
    peers: HashMap<DeviceId, PeerReachability>,
}

impl NetworkStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_overall(&mut self, state: NetworkState) {
        self.overall = Some(state);
    }

    pub fn overall(&self) -> Option<NetworkState> {
        self.overall
    }

    pub fn set_peer(&mut self, device: DeviceId, reachability: PeerReachability) {
        self.peers.insert(device, reachability);
    }

    /// Every peer with a known reachability (for mirroring this state).
    pub fn peers(&self) -> impl Iterator<Item = (DeviceId, PeerReachability)> + '_ {
        self.peers.iter().map(|(d, r)| (*d, *r))
    }

    pub fn peer(&self, device: DeviceId) -> Option<PeerReachability> {
        self.peers.get(&device).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_state_starts_unknown() {
        let status = NetworkStatus::new();
        assert_eq!(status.overall(), None);
    }

    #[test]
    fn per_peer_reachability_is_tracked_independently() {
        let mut status = NetworkStatus::new();
        let alice = DeviceId::new();
        let bob = DeviceId::new();

        status.set_peer(alice, PeerReachability::Connected);
        status.set_peer(bob, PeerReachability::Connecting);

        assert_eq!(status.peer(alice), Some(PeerReachability::Connected));
        assert_eq!(status.peer(bob), Some(PeerReachability::Connecting));
        assert_eq!(status.peer(DeviceId::new()), None);
    }
}
