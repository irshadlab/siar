//! Transport classification and static capability baselines — moved
//! here from the retired `siar-routing::path` (see `MIGRATION.md`,
//! step 5). `siar-routing-policy` deliberately has zero transport
//! dependency, so this glue — which needs `iroh::EndpointAddr` — lives
//! here instead, alongside [`crate::transport_manager`], the one real
//! consumer of it.
//!
//! [`classify_endpoint_addr`] is unchanged in substance from the
//! original — same private/public-IP heuristic, same documented
//! limitations (an advertised LAN IP that was actually reached over
//! relay still classifies as `LocalLan`; superseded entirely by a
//! future `Connection::paths()` integration, not attempted here).
//! Retargeted to return [`siar_routing_policy::TransportKind`] instead
//! of `siar_domain::TransportLink`, since that's what
//! [`siar_routing_policy::PathCandidate::transport`] actually needs;
//! [`crate::ConnectivityMonitor`] (this crate's other, pre-existing
//! consumer of a link classification) keeps using `TransportLink` for
//! its own unrelated up/down purpose — the two call sites don't need
//! to share one enum.
//!
//! [`capabilities_for`] is a **reasoned adaptation, not a verbatim
//! port**: the original's `TransportCapabilities` was a numeric-class
//! struct (`BandwidthClass`/`LatencyClass`) that has no direct home in
//! [`siar_routing_policy::types::PathCapabilities`] (a boolean
//! feature/policy struct — see `MIGRATION.md`'s "shape-superseded by
//! `PathMetrics`'s numeric fields" note for where the numeric side
//! actually belongs, once [`crate::transport_manager::
//! TransportManager`] has something real to put there via
//! `siar_routing_policy::LinkHealth`). What follows maps only the
//! fields `PathCapabilities` actually has, using each field's own
//! documented meaning rather than the original struct's field names.

use siar_routing_policy::types::{MeteredState, PathCapabilities, RoamingState, TransportKind};

pub fn classify_endpoint_addr(addr: &iroh::EndpointAddr) -> TransportKind {
    let mut saw_any_ip = false;
    for socket_addr in addr.ip_addrs() {
        saw_any_ip = true;
        if is_local_or_private_ip(socket_addr) {
            return TransportKind::LocalLan;
        }
    }
    if saw_any_ip {
        TransportKind::IrohDirect
    } else {
        TransportKind::IrohRelay
    }
}

fn is_local_or_private_ip(socket_addr: &std::net::SocketAddr) -> bool {
    match socket_addr.ip() {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        std::net::IpAddr::V6(v6) => {
            // fc00::/7 (unique local) checked by bit pattern rather
            // than `Ipv6Addr::is_unique_local()`, same "always-stable
            // bit check" reasoning the original documented.
            v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// Static per-`TransportKind` baseline — next.md §7's own examples
/// ("Iroh/direct: high bandwidth, low latency, files=yes, video=yes...
/// BLE: very low bandwidth, higher latency, files=restricted,
/// video=no"), re-expressed in `PathCapabilities`'s boolean shape.
/// `metered`/`roaming` are always [`MeteredState::Unknown`]/
/// [`RoamingState::Unknown`] here — same reasoning the original gave
/// for omitting them entirely: they're properties of one specific
/// active connection, not of the transport kind in the abstract. A
/// live caller with a real platform signal for either overrides these
/// on the resulting value; this function only ever supplies the
/// abstract baseline. `requires_foreground` is always `false` — the
/// original predates §86's "Background Restrictions" concept entirely
/// and never modeled it; a caller with a real platform signal sets it
/// directly, same as `metered`/`roaming`.
///
/// [`TransportKind::MeshRelay`]/[`TransportKind::Dtn`] aren't real
/// "transports" in the sense every other variant is — they're
/// `siar-routing-policy`'s own derived/store-and-forward concepts
/// ([`crate::PathCandidate`]s this crate doesn't itself construct via
/// `classify_endpoint_addr`, and a DTN bundle policy respectively).
/// Included here anyway so this function stays total over
/// `TransportKind` rather than panicking on them if a caller ever does
/// hand one in.
pub fn capabilities_for(kind: TransportKind) -> PathCapabilities {
    use TransportKind::*;
    let (reliable_stream, datagram, large_files, realtime_media, peer_discovery, store_and_forward) =
        match kind {
            IrohDirect | IrohRelay => (true, true, true, true, false, false),
            LocalLan | WifiDirect | WifiAware => (true, true, true, true, true, false),
            BluetoothClassic => (true, true, false, false, true, false),
            BluetoothLe => (false, true, false, false, true, false),
            MeshRelay => (true, true, false, false, false, false),
            Dtn => (false, true, false, false, false, true),
        };
    PathCapabilities {
        reliable_stream,
        datagram,
        large_files,
        realtime_media,
        peer_discovery,
        store_and_forward,
        metered: MeteredState::Unknown,
        roaming: RoamingState::Unknown,
        requires_foreground: false,
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

    fn addr_with_ips(id: iroh::EndpointId, ips: &[std::net::SocketAddr]) -> iroh::EndpointAddr {
        let addrs = ips.iter().map(|&ip| iroh::TransportAddr::Ip(ip)).collect();
        iroh::EndpointAddr { id, addrs }
    }

    fn addr_with_relay_only(id: iroh::EndpointId) -> iroh::EndpointAddr {
        let relay_url: iroh::RelayUrl = "https://relay.example.com"
            .parse()
            .expect("valid relay URL");
        iroh::EndpointAddr {
            id,
            addrs: std::collections::BTreeSet::from([iroh::TransportAddr::Relay(relay_url)]),
        }
    }

    #[test]
    fn classify_endpoint_addr_with_a_private_ipv4_is_local_lan() {
        let addr = addr_with_ips(test_endpoint_id(40), &["192.168.1.5:4433".parse().unwrap()]);
        assert_eq!(classify_endpoint_addr(&addr), TransportKind::LocalLan);
    }

    #[test]
    fn classify_endpoint_addr_with_a_public_ipv4_is_iroh_direct() {
        let addr = addr_with_ips(test_endpoint_id(41), &["8.8.8.8:4433".parse().unwrap()]);
        assert_eq!(classify_endpoint_addr(&addr), TransportKind::IrohDirect);
    }

    #[test]
    fn classify_endpoint_addr_with_only_a_relay_url_is_iroh_relay() {
        let addr = addr_with_relay_only(test_endpoint_id(42));
        assert_eq!(classify_endpoint_addr(&addr), TransportKind::IrohRelay);
    }

    #[test]
    fn classify_endpoint_addr_prefers_local_lan_when_both_kinds_of_ip_are_present() {
        let addr = addr_with_ips(
            test_endpoint_id(43),
            &[
                "192.168.1.5:4433".parse().unwrap(),
                "8.8.8.8:4433".parse().unwrap(),
            ],
        );
        assert_eq!(classify_endpoint_addr(&addr), TransportKind::LocalLan);
    }

    #[test]
    fn classify_endpoint_addr_link_local_ipv4_is_local_lan() {
        let addr = addr_with_ips(test_endpoint_id(44), &["169.254.1.1:4433".parse().unwrap()]);
        assert_eq!(classify_endpoint_addr(&addr), TransportKind::LocalLan);
    }

    #[test]
    fn bluetooth_le_supports_neither_large_files_nor_realtime_media() {
        let caps = capabilities_for(TransportKind::BluetoothLe);
        assert!(!caps.large_files);
        assert!(!caps.realtime_media);
    }

    #[test]
    fn dtn_is_the_only_kind_marked_store_and_forward() {
        for kind in [
            TransportKind::IrohDirect,
            TransportKind::IrohRelay,
            TransportKind::LocalLan,
            TransportKind::WifiDirect,
            TransportKind::WifiAware,
            TransportKind::BluetoothClassic,
            TransportKind::BluetoothLe,
            TransportKind::MeshRelay,
        ] {
            assert!(!capabilities_for(kind).store_and_forward);
        }
        assert!(capabilities_for(TransportKind::Dtn).store_and_forward);
    }

    #[test]
    fn metered_and_roaming_are_always_unknown_from_this_static_baseline() {
        let caps = capabilities_for(TransportKind::LocalLan);
        assert_eq!(caps.metered, MeteredState::Unknown);
        assert_eq!(caps.roaming, RoamingState::Unknown);
    }
}
