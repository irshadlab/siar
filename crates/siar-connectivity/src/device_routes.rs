//! Device-to-endpoint hints — moved here from the retired
//! `siar-routing::device_routes` (see `MIGRATION.md`, step 5): this is
//! the transport-address glue `siar-routing-policy` deliberately has
//! no equivalent of (that crate has zero transport dependency by
//! design), so it belongs alongside [`crate::transport_manager`], not
//! inside the routing-policy engine itself.
//!
//! `siar_routing_policy::PathCandidate` keys on `siar_domain::DeviceId`
//! (application/contact identity); every real discovery mechanism this
//! workspace has built — `SiarEndpoint::local_peers`, mDNS, WifiDirect
//! group info — only ever produces an iroh `EndpointId`/`EndpointAddr`
//! (network identity). [`DeviceRoutes`] is the join between the two,
//! same reasoning the original carried: *"If a caller genuinely needs
//! a `DeviceId`-keyed view... that's a join against whatever maps
//! `DeviceId -> EndpointId` for paired contacts."*
//!
//! **New in this pass**: [`DeviceRoutes::device_for`], the reverse
//! lookup the original never needed. The original only ever went
//! `DeviceId -> EndpointId` (a device disclosing its own location via
//! `MailboxCheckIn`). Building [`crate::transport_manager::
//! TransportManager::sync_local_peers`] against `PathCandidate`
//! surfaced the opposite real need: an `EndpointId` freshly observed
//! from mDNS has no `DeviceId` at all until *something* has already
//! recorded which device that endpoint belongs to. This module now
//! keeps both directions in sync from the same `record` call, backed
//! by a second `HashMap` rather than a linear scan, since
//! `sync_local_peers` calls the reverse direction once per observed
//! peer on every sync tick.
//!
//! Unauthenticated, same caveat the original and `MailboxCheckIn`
//! itself already carry — a device claim isn't cryptographically
//! verified anywhere in this pass (next.md §32's real capability/token
//! system, still not attempted), so every hint here is "worth trying,"
//! not "guaranteed correct." **Real, named gap**:
//! `TransportManager::sync_local_peers` can only build a
//! [`crate::candidate_store::PathCandidate`] for an `EndpointId` this
//! table already has a recorded `DeviceId` for — an `EndpointId`
//! observed from mDNS with no prior `MailboxCheckIn`-style disclosure
//! is silently skipped, not guessed at. Closing that gap needs a real
//! integration with `siar-identity-multidevice`'s device-linking/
//! verification flow (which knows a peer's `DeviceId` the moment a
//! handshake completes) — out of scope for this pass, same "named, not
//! silently worked around" posture as everything else this workspace
//! flags rather than fakes.

use iroh::EndpointId;
use siar_domain::DeviceId;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
struct Hint {
    endpoint: EndpointId,
    last_seen: u64,
}

pub struct DeviceRoutes {
    hints: HashMap<DeviceId, Hint>,
    reverse: HashMap<EndpointId, DeviceId>,
}

impl DeviceRoutes {
    pub fn new() -> Self {
        Self {
            hints: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Records or refreshes a device's self-disclosed endpoint —
    /// overwrites any previous hint for the same device outright
    /// rather than keeping history, since a device only has one
    /// current location worth acting on. Also updates the reverse
    /// index; if `endpoint` was previously associated with a
    /// *different* device (a re-used or re-assigned endpoint — rare,
    /// but not impossible on a network identity that can churn), that
    /// stale reverse mapping is corrected to point at `device`.
    pub fn record(&mut self, device: DeviceId, endpoint: EndpointId, now: u64) {
        if let Some(previous) = self.hints.insert(
            device,
            Hint {
                endpoint,
                last_seen: now,
            },
        ) {
            if previous.endpoint != endpoint {
                self.reverse.remove(&previous.endpoint);
            }
        }
        self.reverse.insert(endpoint, device);
    }

    pub fn get(&self, device: DeviceId) -> Option<EndpointId> {
        self.hints.get(&device).map(|hint| hint.endpoint)
    }

    /// The reverse direction — which device (if any) has disclosed
    /// `endpoint` as its own. `None` for an endpoint this table has no
    /// disclosure for yet, which is the expected answer for most
    /// freshly-observed mDNS peers (see this module's top doc comment).
    pub fn device_for(&self, endpoint: EndpointId) -> Option<DeviceId> {
        self.reverse.get(&endpoint).copied()
    }

    pub fn remove_stale(&mut self, now: u64, max_age: u64) {
        let reverse = &mut self.reverse;
        self.hints.retain(|_, hint| {
            let keep = now.saturating_sub(hint.last_seen) <= max_age;
            if !keep {
                reverse.remove(&hint.endpoint);
            }
            keep
        });
    }
}

impl Default for DeviceRoutes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same test-fixture construction as the original `siar-routing`
    // module's own tests — see that module's history for why this
    // specific `SecretKey::from_str` shape was used instead of a
    // guessed `from_bytes` method.
    fn test_endpoint_id(seed: u8) -> EndpointId {
        use std::str::FromStr;
        let hex = format!("{seed:02x}").repeat(32);
        let secret = iroh::SecretKey::from_str(&hex).expect("valid 64-char hex test secret key");
        secret.public()
    }

    #[test]
    fn record_then_get_round_trips() {
        let mut routes = DeviceRoutes::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(1);
        routes.record(device, endpoint, 100);
        assert_eq!(routes.get(device), Some(endpoint));
    }

    #[test]
    fn get_on_unknown_device_is_none() {
        let routes = DeviceRoutes::new();
        assert_eq!(routes.get(DeviceId::new()), None);
    }

    #[test]
    fn a_second_record_overwrites_rather_than_accumulating() {
        let mut routes = DeviceRoutes::new();
        let device = DeviceId::new();
        routes.record(device, test_endpoint_id(1), 100);
        routes.record(device, test_endpoint_id(2), 200);
        assert_eq!(routes.get(device), Some(test_endpoint_id(2)));
    }

    #[test]
    fn remove_stale_drops_old_hints_and_keeps_recent_ones() {
        let mut routes = DeviceRoutes::new();
        let old_device = DeviceId::new();
        let recent_device = DeviceId::new();
        routes.record(old_device, test_endpoint_id(1), 0);
        routes.record(recent_device, test_endpoint_id(2), 90);

        routes.remove_stale(100, 50);
        assert_eq!(routes.get(old_device), None);
        assert_eq!(routes.get(recent_device), Some(test_endpoint_id(2)));
    }

    #[test]
    fn device_for_is_the_reverse_of_record() {
        let mut routes = DeviceRoutes::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(3);
        routes.record(device, endpoint, 0);
        assert_eq!(routes.device_for(endpoint), Some(device));
    }

    #[test]
    fn device_for_is_none_for_an_undisclosed_endpoint() {
        let routes = DeviceRoutes::new();
        assert_eq!(routes.device_for(test_endpoint_id(4)), None);
    }

    #[test]
    fn re_recording_a_device_at_a_new_endpoint_clears_the_old_reverse_entry() {
        let mut routes = DeviceRoutes::new();
        let device = DeviceId::new();
        let first_endpoint = test_endpoint_id(5);
        let second_endpoint = test_endpoint_id(6);
        routes.record(device, first_endpoint, 0);
        routes.record(device, second_endpoint, 10);
        assert_eq!(routes.device_for(first_endpoint), None);
        assert_eq!(routes.device_for(second_endpoint), Some(device));
    }

    #[test]
    fn remove_stale_also_clears_the_reverse_index() {
        let mut routes = DeviceRoutes::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(7);
        routes.record(device, endpoint, 0);
        routes.remove_stale(100, 50);
        assert_eq!(routes.device_for(endpoint), None);
    }
}
