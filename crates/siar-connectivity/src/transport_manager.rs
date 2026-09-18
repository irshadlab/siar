//! next.md §90's `TransportManager`, rewritten against
//! `siar-routing-policy` (see `MIGRATION.md`, step 5) — this crate's
//! prior version built it on the now-retired `siar-routing::PathTable`.
//!
//! **What changed, and why call sites elsewhere in this workspace will
//! need small updates**: the old `PathTable` keyed on `iroh::EndpointId`
//! (network identity) directly; `siar_routing_policy::PathCandidate`
//! keys on `siar_domain::DeviceId` (application/contact identity) by
//! design (see that crate's own `types.rs` doc comments). This isn't a
//! cosmetic rename — it's the real identity-mismatch reconciliation
//! `MIGRATION.md` was written to resolve, and it means a caller that
//! only ever knew an `EndpointId` (mDNS, a live connection) now needs
//! [`crate::device_routes::DeviceRoutes`] to resolve one before this
//! type can do anything useful with it. **Real, named gap** (see
//! [`crate::device_routes`]'s own top doc comment for the full
//! reasoning): an `EndpointId` this table has no recorded `DeviceId`
//! for is silently skipped, not guessed at — closing that needs a real
//! `siar-identity-multidevice` integration, out of scope here.
//!
//! Candidates are keyed by `(DeviceId, TransportKind)` rather than a
//! single `PathCandidate` per device — a peer reachable over both LAN
//! and Bluetooth genuinely has two distinct candidates, same as the
//! old `PathTable` keeping one `PathEntry` per `(destination, link)`
//! pair.
//!
//! Split into [`CandidateTable`] (pure bookkeeping, no I/O — every
//! method below is real, direct-tested logic) and [`TransportManager`]
//! (the thin wrapper actually holding a live [`SiarEndpoint`]). The
//! original bundled both into one type; splitting them here is what
//! makes real unit tests possible at all — `SiarEndpoint::bind` is
//! async and binds a real socket, so a test can't cheaply construct
//! one just to exercise `record_send_outcome`'s bookkeeping, which
//! never actually touches the endpoint.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use siar_domain::DeviceId;
use siar_routing_policy::acquisition::CandidateState;
use siar_routing_policy::candidate::{PathCandidate, TransportEndpoint};
use siar_routing_policy::link_health::{health_from_reliability, LinkHealth, SendOutcome};
use siar_routing_policy::metrics::PathMetrics;
use siar_routing_policy::types::{PathId, RouteHealth, TransportKind};
use siar_transport::SiarEndpoint;

use crate::candidate_source::{capabilities_for, classify_endpoint_addr};
use crate::device_routes::DeviceRoutes;

/// Default rolling-window size for each `(DeviceId, TransportKind)`'s
/// [`LinkHealth`] — same reasoning and same "not tuned against real
/// usage data, a starting point" status the original constant carried.
const LINK_HEALTH_WINDOW: usize = 20;

/// Pure candidate/health bookkeeping — no I/O, no endpoint, fully
/// unit-testable on its own.
pub struct CandidateTable {
    device_routes: Mutex<DeviceRoutes>,
    candidates: Mutex<HashMap<(DeviceId, TransportKind), PathCandidate>>,
    link_health: Mutex<HashMap<(DeviceId, TransportKind), LinkHealth>>,
    /// When each candidate was last (re)confirmed by
    /// [`Self::sync_local_peers`] — a real, named gap in this crate's
    /// first pass (`MIGRATION.md` step 5): the old `PathTable::
    /// remove_stale` actively evicted entries no longer refreshed by
    /// mDNS, but nothing tracked an equivalent timestamp on
    /// `PathCandidate` itself (it has no such field, by that crate's
    /// own design — `siar_routing_policy::PathCandidate` doesn't know
    /// about wall-clock observation recency at all). Tracked here,
    /// alongside the table that actually needs it, rather than by
    /// adding a field to `PathCandidate` for one caller's bookkeeping.
    last_observed: Mutex<HashMap<(DeviceId, TransportKind), u64>>,
}

impl CandidateTable {
    pub fn new() -> Self {
        Self {
            device_routes: Mutex::new(DeviceRoutes::new()),
            candidates: Mutex::new(HashMap::new()),
            link_health: Mutex::new(HashMap::new()),
            last_observed: Mutex::new(HashMap::new()),
        }
    }

    /// A device disclosing its own current endpoint (next.md's
    /// `MailboxCheckIn` — see [`crate::device_routes`]'s doc comment)
    /// — the one real signal this table has for the `EndpointId ->
    /// DeviceId` join [`Self::sync_local_peers`] depends on. A caller
    /// forwards a real check-in's claimed identity here as it arrives.
    pub fn record_device_endpoint(&self, device: DeviceId, endpoint: iroh::EndpointId, now: u64) {
        self.device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned")
            .record(device, endpoint, now);
    }

    /// The `EndpointId -> DeviceId` resolution [`Self::sync_local_peers`]
    /// itself relies on, exposed for a caller that needs the same
    /// resolution for its own purposes (e.g. resolving the advertiser
    /// or the claimed destination of a received routing advertisement
    /// into a `DeviceId` before it can be composed into a
    /// [`PathCandidate`] at all — see `siar_routing_policy::
    /// relay_composition`). Deliberately the *same* underlying
    /// [`DeviceRoutes`] `sync_local_peers` uses, not a second instance
    /// a caller would otherwise have to keep in sync by hand.
    pub fn device_for(&self, endpoint: iroh::EndpointId) -> Option<DeviceId> {
        self.device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned")
            .device_for(endpoint)
    }

    /// The forward direction of [`Self::device_for`] — `device`'s
    /// last self-disclosed endpoint, if this table has recorded one
    /// (via [`Self::record_device_endpoint`]). What a caller needing
    /// to push something to a known destination (rather than merely
    /// resolve an observed peer's identity) actually needs.
    pub fn known_endpoint_for(&self, device: DeviceId) -> Option<iroh::EndpointId> {
        self.device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned")
            .get(device)
    }

    /// Refreshes `LocalLan`/`IrohDirect`/`IrohRelay` candidates from a
    /// caller-supplied list of observed peers (in practice,
    /// `SiarEndpoint::local_peers()`, via
    /// [`TransportManager::sync_local_peers`]) — for each peer whose
    /// `EndpointId` this table can already resolve to a `DeviceId` via
    /// [`crate::device_routes::DeviceRoutes`] (see this module's top
    /// doc comment for the peers it cannot yet resolve, and why). `now`
    /// is this call's own opaque tick, same "caller supplies real-world
    /// timing" split every other clock-adjacent type in this workspace
    /// already uses.
    pub fn sync_local_peers(&self, observed: &[iroh::EndpointAddr], now: u64) {
        let device_routes = self
            .device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned");
        let mut candidates = self.candidates.lock().expect("candidates lock poisoned");
        let mut last_observed = self
            .last_observed
            .lock()
            .expect("last_observed lock poisoned");
        for addr in observed {
            let Some(device) = device_routes.device_for(addr.id) else {
                // Real, named gap — see this module's top doc comment.
                continue;
            };
            let kind = classify_endpoint_addr(addr);
            candidates.insert(
                (device, kind),
                PathCandidate {
                    path_id: PathId::new(),
                    transport: kind,
                    peer: device,
                    endpoint: TransportEndpoint(addr.id.as_bytes().to_vec()),
                    metrics: PathMetrics::unknown(),
                    capabilities: capabilities_for(kind),
                    // "iroh's own mDNS/discovery just told us this
                    // peer is reachable right now" — a reasonable
                    // prior for a link currently confirmed up, not a
                    // measurement; same reasoning the original gave
                    // for defaulting `reliability` to `1.0` rather
                    // than an "unknown" placeholder.
                    health: RouteHealth::Healthy,
                    underlay: None,
                    state: CandidateState::Active,
                },
            );
            last_observed.insert((device, kind), now);
        }
    }

    /// Drops any candidate not reconfirmed by [`Self::sync_local_peers`]
    /// within the last `max_age` — next.md §92's "mobile topology
    /// changes too quickly" reasoning, the same one the retired
    /// `PathTable::remove_stale`'s own doc comment already gave. Also
    /// delegates to [`DeviceRoutes::remove_stale`] with the same
    /// `max_age`, since a device's self-disclosed endpoint hint and a
    /// candidate built from mDNS observation go stale for the same
    /// underlying reason.
    pub fn remove_stale(&self, now: u64, max_age: u64) {
        let mut candidates = self.candidates.lock().expect("candidates lock poisoned");
        let mut last_observed = self
            .last_observed
            .lock()
            .expect("last_observed lock poisoned");
        let mut link_health = self
            .link_health
            .lock()
            .expect("LinkHealth map lock poisoned");
        last_observed.retain(|key, &mut observed_at| {
            let keep = now.saturating_sub(observed_at) <= max_age;
            if !keep {
                candidates.remove(key);
                link_health.remove(key);
            }
            keep
        });
        drop((candidates, last_observed, link_health));
        self.device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned")
            .remove_stale(now, max_age);
    }

    /// Every current candidate for `device`, across every transport
    /// this table has observed it on.
    pub fn candidates_for(&self, device: DeviceId) -> Vec<PathCandidate> {
        self.candidates
            .lock()
            .expect("candidates lock poisoned")
            .values()
            .filter(|candidate| candidate.peer == device)
            .cloned()
            .collect()
    }

    /// Direct access to the full candidate table, for a caller that
    /// wants to feed everything currently known into
    /// `siar_routing_policy::decide_route`/`plan_route` itself, rather
    /// than going through [`Self::candidates_for`] one device at a
    /// time.
    pub fn candidates(&self) -> MutexGuard<'_, HashMap<(DeviceId, TransportKind), PathCandidate>> {
        self.candidates.lock().expect("candidates lock poisoned")
    }

    /// Folds one real send attempt into this `(device, kind)`'s
    /// rolling [`LinkHealth`] window, then updates that candidate's
    /// `metrics`/`health` in place — closing next.md §90's "actually
    /// measuring" gap on the computation side. `destination` is the
    /// `EndpointId` a caller actually sent to; it's resolved to a
    /// `DeviceId` the same way [`Self::sync_local_peers`] does.
    /// Silently does nothing if either that resolution fails, or if no
    /// candidate for the resulting `(device, kind)` exists yet — a
    /// measurement can only enrich a candidate this table already has
    /// enough context (capabilities, endpoint bytes) to have
    /// constructed; it can't conjure one from an outcome alone.
    ///
    /// No real caller exists yet (same status the original method
    /// carried) — this is the landing point for one, not evidence one
    /// is wired in.
    pub fn record_send_outcome(
        &self,
        destination: iroh::EndpointId,
        kind: TransportKind,
        outcome: SendOutcome,
    ) {
        let Some(device) = self
            .device_routes
            .lock()
            .expect("DeviceRoutes lock poisoned")
            .device_for(destination)
        else {
            return;
        };

        let mut candidates = self.candidates.lock().expect("candidates lock poisoned");
        let Some(candidate) = candidates.get_mut(&(device, kind)) else {
            return;
        };

        let mut health = self
            .link_health
            .lock()
            .expect("LinkHealth map lock poisoned");
        let link_health = health
            .entry((device, kind))
            .or_insert_with(|| LinkHealth::new(LINK_HEALTH_WINDOW));
        link_health.record_outcome(outcome);
        link_health.apply_to(&mut candidate.metrics);
        candidate.health = health_from_reliability(link_health.reliability());
    }
}

impl Default for CandidateTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TransportManager {
    endpoint: Arc<SiarEndpoint>,
    table: CandidateTable,
}

impl TransportManager {
    pub fn new(endpoint: Arc<SiarEndpoint>) -> Self {
        Self {
            endpoint,
            table: CandidateTable::new(),
        }
    }

    pub fn record_device_endpoint(&self, device: DeviceId, endpoint: iroh::EndpointId, now: u64) {
        self.table.record_device_endpoint(device, endpoint, now);
    }

    pub fn device_for(&self, endpoint: iroh::EndpointId) -> Option<DeviceId> {
        self.table.device_for(endpoint)
    }

    pub fn known_endpoint_for(&self, device: DeviceId) -> Option<iroh::EndpointId> {
        self.table.known_endpoint_for(device)
    }

    pub fn sync_local_peers(&self, now: u64) {
        self.table
            .sync_local_peers(&self.endpoint.local_peers(), now);
    }

    pub fn remove_stale(&self, now: u64, max_age: u64) {
        self.table.remove_stale(now, max_age);
    }

    pub fn candidates_for(&self, device: DeviceId) -> Vec<PathCandidate> {
        self.table.candidates_for(device)
    }

    pub fn candidates(&self) -> MutexGuard<'_, HashMap<(DeviceId, TransportKind), PathCandidate>> {
        self.table.candidates()
    }

    pub fn record_send_outcome(
        &self,
        destination: iroh::EndpointId,
        kind: TransportKind,
        outcome: SendOutcome,
    ) {
        self.table.record_send_outcome(destination, kind, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_endpoint_id(seed: u8) -> iroh::EndpointId {
        use std::str::FromStr;
        let hex = format!("{seed:02x}").repeat(32);
        let secret = iroh::SecretKey::from_str(&hex).expect("valid 64-char hex test secret key");
        secret.public()
    }

    fn candidate(device: DeviceId, kind: TransportKind) -> PathCandidate {
        PathCandidate {
            path_id: PathId::new(),
            transport: kind,
            peer: device,
            endpoint: TransportEndpoint(vec![1]),
            metrics: PathMetrics::unknown(),
            capabilities: capabilities_for(kind),
            health: RouteHealth::Healthy,
            underlay: None,
            state: CandidateState::Active,
        }
    }

    #[test]
    fn sync_local_peers_skips_endpoints_with_no_known_device() {
        let table = CandidateTable::new();
        let addr = iroh::EndpointAddr {
            id: test_endpoint_id(1),
            addrs: Default::default(),
        };
        table.sync_local_peers(&[addr], 0);
        assert!(table.candidates().is_empty());
    }

    #[test]
    fn sync_local_peers_builds_a_candidate_for_a_resolvable_endpoint() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(2);
        table.record_device_endpoint(device, endpoint, 0);
        let addr = iroh::EndpointAddr {
            id: endpoint,
            addrs: std::collections::BTreeSet::from([iroh::TransportAddr::Ip(
                "192.168.1.5:4433".parse().unwrap(),
            )]),
        };
        table.sync_local_peers(&[addr], 10);
        let candidates = table.candidates_for(device);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].transport, TransportKind::LocalLan);
    }

    #[test]
    fn record_send_outcome_does_nothing_with_no_known_device_for_the_endpoint() {
        let table = CandidateTable::new();
        table.record_send_outcome(
            test_endpoint_id(3),
            TransportKind::LocalLan,
            SendOutcome::success(20),
        );
        assert!(table.candidates().is_empty());
    }

    #[test]
    fn record_send_outcome_does_nothing_with_no_existing_candidate() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(4);
        table.record_device_endpoint(device, endpoint, 0);
        // Device is known, but no candidate for (device, LocalLan) has
        // ever been constructed (no sync_local_peers call happened).
        table.record_send_outcome(endpoint, TransportKind::LocalLan, SendOutcome::success(20));
        assert!(table.candidates_for(device).is_empty());
    }

    #[test]
    fn record_send_outcome_updates_an_existing_candidates_metrics_and_health() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(5);
        table.record_device_endpoint(device, endpoint, 0);
        table.candidates().insert(
            (device, TransportKind::LocalLan),
            candidate(device, TransportKind::LocalLan),
        );

        for _ in 0..3 {
            table.record_send_outcome(endpoint, TransportKind::LocalLan, SendOutcome::success(40));
        }

        let candidates = table.candidates_for(device);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].metrics.rtt_millis, Some(40));
        assert_eq!(candidates[0].health, RouteHealth::Healthy);
    }

    #[test]
    fn record_send_outcome_only_touches_the_matching_transport_kind() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(6);
        table.record_device_endpoint(device, endpoint, 0);
        table.candidates().insert(
            (device, TransportKind::BluetoothLe),
            candidate(device, TransportKind::BluetoothLe),
        );
        // Reports an outcome for LocalLan, which has no candidate —
        // the BluetoothLe candidate must be left untouched.
        table.record_send_outcome(endpoint, TransportKind::LocalLan, SendOutcome::success(10));
        let candidates = table.candidates_for(device);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].metrics.rtt_millis, None);
    }

    #[test]
    fn repeated_failures_degrade_health_from_healthy() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(7);
        table.record_device_endpoint(device, endpoint, 0);
        table.candidates().insert(
            (device, TransportKind::LocalLan),
            candidate(device, TransportKind::LocalLan),
        );

        for _ in 0..5 {
            table.record_send_outcome(endpoint, TransportKind::LocalLan, SendOutcome::failure());
        }

        let candidates = table.candidates_for(device);
        assert_eq!(candidates[0].health, RouteHealth::Unreachable);
    }

    #[test]
    fn device_for_resolves_the_same_disclosures_sync_local_peers_uses() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(8);
        table.record_device_endpoint(device, endpoint, 0);
        assert_eq!(table.device_for(endpoint), Some(device));
        assert_eq!(table.device_for(test_endpoint_id(9)), None);
        assert_eq!(table.known_endpoint_for(device), Some(endpoint));
    }

    #[test]
    fn remove_stale_drops_candidates_not_reconfirmed_by_sync_local_peers() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(10);
        table.record_device_endpoint(device, endpoint, 0);
        let addr = iroh::EndpointAddr {
            id: endpoint,
            addrs: std::collections::BTreeSet::from([iroh::TransportAddr::Ip(
                "192.168.1.5:4433".parse().unwrap(),
            )]),
        };
        table.sync_local_peers(&[addr], 0);
        assert_eq!(table.candidates_for(device).len(), 1);

        table.remove_stale(1000, 500);
        assert!(
            table.candidates_for(device).is_empty(),
            "a candidate not reconfirmed within max_age should be dropped"
        );
    }

    #[test]
    fn remove_stale_keeps_candidates_reconfirmed_within_max_age() {
        let table = CandidateTable::new();
        let device = DeviceId::new();
        let endpoint = test_endpoint_id(11);
        table.record_device_endpoint(device, endpoint, 0);
        let addr = iroh::EndpointAddr {
            id: endpoint,
            addrs: std::collections::BTreeSet::from([iroh::TransportAddr::Ip(
                "192.168.1.5:4433".parse().unwrap(),
            )]),
        };
        table.sync_local_peers(&[addr], 900);

        table.remove_stale(1000, 500);
        assert_eq!(table.candidates_for(device).len(), 1);
    }
}
