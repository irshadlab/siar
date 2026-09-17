//! Ties Phase 1's `SiarEndpoint::local_peers()` and Phase 2's
//! `WifiDirectBridge::group_info()` into a shared
//! `siar_domain::ConnectivityState` — the wiring both of those phases'
//! doc comments flagged as missing ("no `apps/android` crate exists yet
//! to own that state").
//!
//! Scoped to exactly the two links those phases actually built
//! introspection for:
//!
//! - **`LocalLan`** — up whenever [`SiarEndpoint::local_peers`] is
//!   non-empty. Works on every platform, since `SiarEndpoint` itself
//!   does.
//! - **`WifiDirect`** (Android only) — up whenever
//!   [`siar_transport_wifi_direct::group_info`] returns `Some`.
//!
//! What's deliberately NOT here:
//!
//! - **Internet** (`InternetDirect`/`InternetRelay`) — `SiarEndpoint`
//!   doesn't expose iroh's own connectivity status as a method yet,
//!   and guessing at what that method would be named or return is
//!   exactly the kind of unverified-against-real-docs risk this
//!   workspace has avoided everywhere else (see `local_discovery.rs`'s
//!   doc comment on why the mDNS API itself was looked up directly
//!   rather than assumed). Real next step, not attempted here.
//! - **Wi-Fi Aware, Bluetooth Classic, BLE** — no crate exists yet with
//!   anything to poll for these. Wi-Fi Aware has no crate at all yet;
//!   BLE's `siar-transport-ble` (Phase 3) built the fragment/reassembly
//!   protocol, and `siar-transport-ble-android` now has the GATT JNI
//!   boundary too, but neither reports a "link is up" style status yet
//!   the way `WifiDirectBridge::group_info` does — BLE's connections
//!   are per-peer (`BleLinkBridge`), not a single radio-wide state, so
//!   "is BLE up" isn't a single yes/no the way the other links are.
//!
//! [`transport_manager`] is this crate's other half: wiring
//! `SiarEndpoint::local_peers()` into a live
//! `siar_routing_policy::PathCandidate` table, not just a
//! `ConnectivityState` boolean.

pub mod candidate_source;
pub mod device_routes;
pub mod transport_manager;
pub use transport_manager::{CandidateTable, TransportManager};

use std::sync::Arc;

use siar_domain::{ConnectivityState, TransportLink};
use siar_transport::SiarEndpoint;

#[cfg(target_os = "android")]
use std::sync::Mutex;

pub struct ConnectivityMonitor {
    endpoint: Arc<SiarEndpoint>,
    #[cfg(target_os = "android")]
    wifi_direct: Option<Arc<Mutex<siar_transport_wifi_direct::WifiDirectBridge>>>,
}

impl ConnectivityMonitor {
    pub fn new(endpoint: Arc<SiarEndpoint>) -> Self {
        Self {
            endpoint,
            #[cfg(target_os = "android")]
            wifi_direct: None,
        }
    }

    /// Registers the app's `WifiDirectBridge` handle so `snapshot` can
    /// poll it. Optional — a desktop/CLI build has no Wi-Fi Direct
    /// bridge to register in the first place, and `snapshot` on a
    /// monitor without one just never marks `WifiDirect` up, which is
    /// the correct answer for those builds anyway.
    #[cfg(target_os = "android")]
    pub fn with_wifi_direct(
        mut self,
        bridge: Arc<Mutex<siar_transport_wifi_direct::WifiDirectBridge>>,
    ) -> Self {
        self.wifi_direct = Some(bridge);
        self
    }

    /// Recomputes connectivity from scratch. Cheap enough to call on a
    /// timer — this doesn't own or spawn one itself; a caller (once
    /// `apps/android` or `apps/desktop` has an event loop to hang it
    /// off) decides the polling interval, same "caller supplies the
    /// real-world timing, this stays pure computation over whatever
    /// state exists right now" split every other clock-adjacent type in
    /// this workspace already uses.
    pub fn snapshot(&self) -> ConnectivityState {
        let mut state = ConnectivityState::new();

        if !self.endpoint.local_peers().is_empty() {
            state.mark_up(TransportLink::LocalLan);
        }

        #[cfg(target_os = "android")]
        if let Some(bridge) = &self.wifi_direct {
            if siar_transport_wifi_direct::group_info(bridge).is_some() {
                state.mark_up(TransportLink::WifiDirect);
            }
        }

        state
    }
}
