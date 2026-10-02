//! §23 "Spray-and-Wait Baseline", §24 "Epidemic Routing" (explicitly
//! rejected as the default — see [`ForwardingDecision`]'s own doc
//! comment), §25 "Direct Delivery", §26 "Gateway Delivery": the actual
//! decision logic this crate's own `lib.rs` previously listed under
//! "not attempted" — [`crate::types::ForwardingClass`] existed as a
//! value a bundle carried; nothing read it. This module does.

use crate::bundle::DtnBundle;
use crate::spray::spray_allocation;
use crate::types::{DtnDestination, ForwardingClass, RouteToken};

/// A peer this device has just encountered — deliberately abstract
/// (see this crate's own top doc comment: no encounter-protocol/
/// transport integration). A real caller with an actual BLE/Wi-Fi/Iroh
/// encounter would construct one of these per peer it just connected
/// to; nothing here assumes how that encounter happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncounteredPeer {
    pub route_token: RouteToken,
    /// §26: "reliable Internet or destination reachability."
    pub is_gateway: bool,
}

/// §23/§25/§26's three real outcomes, plus the case none of them apply.
/// Deliberately no `Epidemic` variant — §24 rejects epidemic routing
/// as the *default* strategy outright ("send everything to everyone…
/// unsuitable… battery drain, bandwidth waste, storage explosion,
/// metadata leakage") and only allows it for "tightly bounded critical
/// emergency classes, if at all" — a policy decision this module
/// doesn't make unilaterally by adding a variant for it; a caller that
/// really wants that behavior for `DtnPriority::Sos` can already
/// construct it directly from `Spray` with every encountered peer as a
/// target, without this module needing a special case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardingDecision {
    /// §25: "This always outranks relay forwarding" — checked first,
    /// before `forwarding_class` is even consulted, in
    /// [`decide_forwarding`].
    DeliverDirect { to: RouteToken },
    /// §26.
    ForwardToGateway { to: RouteToken },
    /// §23. `retain` is what [`crate::bundle::DtnBundle::consume_replication`]
    /// should be called with afterward — this function only decides
    /// *how many* copies to spray and to whom, it doesn't itself mutate
    /// the bundle (the caller owns that, same "decide, don't mutate"
    /// split `siar_routing_policy::scoring`-equivalent modules in this workspace's
    /// sibling crates already use).
    Spray {
        targets: Vec<RouteToken>,
        retain: u8,
    },
    /// Expired (§20), budget exhausted, or no eligible peer at all —
    /// nothing to do with this bundle right now.
    Hold,
}

/// §16's `DtnDestination` opaque forms are compared by raw token
/// equality — real matching, not a stub. `LocalBroadcast` matches any
/// encountered peer unconditionally: a broadcast bundle has no single
/// "destination token" to compare against; every peer within its
/// radius is a valid direct recipient by definition. Whether a peer is
/// actually within `radius_hops` is this crate's own hop-count
/// bookkeeping ([`DtnBundle::hop_limit`]), not something this function
/// re-derives from the `BroadcastScope` value itself.
pub(crate) fn peer_is_destination(peer_token: &RouteToken, destination: &DtnDestination) -> bool {
    match destination {
        DtnDestination::DeviceOpaque(token)
        | DtnDestination::AccountOpaque(token)
        | DtnDestination::GroupOpaque(token) => peer_token == token,
        DtnDestination::LocalBroadcast(_) => true,
    }
}

/// §189 Phase 4's own named piece, made real. Evaluation order:
/// 1. §20: an expired bundle is never forwarded, full stop.
/// 2. §25: direct delivery to an encountered destination always wins,
///    regardless of `forwarding_class` — even a `DirectOnly` bundle
///    takes this path (it's the *only* path that class ever takes),
///    and even a `SprayAndWait`/`GatewayPreferred` bundle takes this
///    path in preference to spraying/gatewaying when the destination
///    itself is right there.
/// 3. Otherwise, branch on `forwarding_class`:
///    - `DirectOnly`: no destination present → [`ForwardingDecision::Hold`],
///      never relay through anyone else — that's the entire meaning of
///      "direct only."
///    - `GatewayPreferred`: a gateway peer present → forward to it
///      (§26); none present → falls back to `SprayAndWait`'s own
///      behavior against whatever peers *are* present, rather than
///      holding — the spec names the preference but doesn't specify a
///      no-gateway fallback, so "don't just give up" is this module's
///      own reasonable choice, not a transcription.
///    - `SprayAndWait`: real allocation via
///      [`spray_allocation`] (§23, already built, now actually called)
///      against however many peers were encountered.
pub fn decide_forwarding(
    bundle: &DtnBundle,
    encountered_peers: &[EncounteredPeer],
    now_millis: u64,
) -> ForwardingDecision {
    if bundle.is_expired(now_millis) {
        return ForwardingDecision::Hold;
    }

    if let Some(destination_peer) = encountered_peers
        .iter()
        .find(|p| peer_is_destination(&p.route_token, &bundle.destination))
    {
        return ForwardingDecision::DeliverDirect {
            to: destination_peer.route_token.clone(),
        };
    }

    match bundle.forwarding_class {
        ForwardingClass::DirectOnly => ForwardingDecision::Hold,
        ForwardingClass::GatewayPreferred => {
            if let Some(gateway) = encountered_peers.iter().find(|p| p.is_gateway) {
                ForwardingDecision::ForwardToGateway {
                    to: gateway.route_token.clone(),
                }
            } else {
                spray_decision(bundle, encountered_peers)
            }
        }
        ForwardingClass::SprayAndWait => spray_decision(bundle, encountered_peers),
    }
}

fn spray_decision(bundle: &DtnBundle, encountered_peers: &[EncounteredPeer]) -> ForwardingDecision {
    if encountered_peers.is_empty() {
        return ForwardingDecision::Hold;
    }
    let (spray_count, retain) = spray_allocation(
        bundle.replication_budget,
        u8::try_from(encountered_peers.len()).unwrap_or(u8::MAX),
    );
    if spray_count == 0 {
        return ForwardingDecision::Hold;
    }
    let targets = encountered_peers
        .iter()
        .take(spray_count as usize)
        .map(|p| p.route_token.clone())
        .collect();
    ForwardingDecision::Spray { targets, retain }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleIntegrity;
    use crate::payload::PayloadReference;
    use crate::types::{BundleId, DtnPriority, DtnSource, PayloadTypeId};

    fn bundle(
        destination: DtnDestination,
        forwarding_class: ForwardingClass,
        expires_at_millis: u64,
        replication_budget: u8,
    ) -> DtnBundle {
        DtnBundle {
            bundle_id: BundleId::new(),
            source: DtnSource(RouteToken(vec![0])),
            destination,
            payload_type: PayloadTypeId(1),
            created_at_millis: 0,
            expires_at_millis,
            priority: DtnPriority::Normal,
            hop_limit: 4,
            replication_budget,
            forwarding_class,
            payload_ref: PayloadReference::Inline(vec![9]),
            integrity: BundleIntegrity {
                payload_hash: [0u8; 32],
                origin_signature: None,
            },
        }
    }

    fn peer(token: u8, is_gateway: bool) -> EncounteredPeer {
        EncounteredPeer {
            route_token: RouteToken(vec![token]),
            is_gateway,
        }
    }

    #[test]
    fn an_expired_bundle_is_always_held_regardless_of_encountered_peers() {
        let dest_token = RouteToken(vec![7]);
        let b = bundle(
            DtnDestination::DeviceOpaque(dest_token.clone()),
            ForwardingClass::SprayAndWait,
            500,
            4,
        );
        let peers = vec![EncounteredPeer {
            route_token: dest_token,
            is_gateway: false,
        }];
        assert_eq!(decide_forwarding(&b, &peers, 999), ForwardingDecision::Hold);
    }

    #[test]
    fn direct_delivery_always_wins_even_for_spray_and_wait() {
        let dest_token = RouteToken(vec![7]);
        let b = bundle(
            DtnDestination::DeviceOpaque(dest_token.clone()),
            ForwardingClass::SprayAndWait,
            10_000,
            4,
        );
        let peers = vec![
            peer(1, false),
            EncounteredPeer {
                route_token: dest_token.clone(),
                is_gateway: false,
            },
            peer(2, true),
        ];
        assert_eq!(
            decide_forwarding(&b, &peers, 0),
            ForwardingDecision::DeliverDirect { to: dest_token }
        );
    }

    #[test]
    fn direct_only_never_relays_when_destination_is_absent() {
        let b = bundle(
            DtnDestination::DeviceOpaque(RouteToken(vec![7])),
            ForwardingClass::DirectOnly,
            10_000,
            4,
        );
        let peers = vec![peer(1, false), peer(2, true)];
        assert_eq!(decide_forwarding(&b, &peers, 0), ForwardingDecision::Hold);
    }

    #[test]
    fn gateway_preferred_forwards_to_a_gateway_when_one_is_present() {
        let b = bundle(
            DtnDestination::DeviceOpaque(RouteToken(vec![7])),
            ForwardingClass::GatewayPreferred,
            10_000,
            4,
        );
        let peers = vec![peer(1, false), peer(2, true)];
        assert_eq!(
            decide_forwarding(&b, &peers, 0),
            ForwardingDecision::ForwardToGateway {
                to: RouteToken(vec![2])
            }
        );
    }

    #[test]
    fn gateway_preferred_falls_back_to_spraying_when_no_gateway_is_present() {
        let b = bundle(
            DtnDestination::DeviceOpaque(RouteToken(vec![7])),
            ForwardingClass::GatewayPreferred,
            10_000,
            4,
        );
        let peers = vec![peer(1, false), peer(2, false)];
        let decision = decide_forwarding(&b, &peers, 0);
        assert!(matches!(decision, ForwardingDecision::Spray { .. }));
    }

    #[test]
    fn spray_and_wait_sprays_to_available_peers_respecting_the_budget() {
        let b = bundle(
            DtnDestination::DeviceOpaque(RouteToken(vec![7])),
            ForwardingClass::SprayAndWait,
            10_000,
            1,
        );
        let peers = vec![peer(1, false), peer(2, false), peer(3, false)];
        let decision = decide_forwarding(&b, &peers, 0);
        assert_eq!(
            decision,
            ForwardingDecision::Spray {
                targets: vec![RouteToken(vec![1])],
                retain: 0
            }
        );
    }

    #[test]
    fn no_encountered_peers_at_all_holds() {
        let b = bundle(
            DtnDestination::DeviceOpaque(RouteToken(vec![7])),
            ForwardingClass::SprayAndWait,
            10_000,
            4,
        );
        assert_eq!(decide_forwarding(&b, &[], 0), ForwardingDecision::Hold);
    }

    #[test]
    fn local_broadcast_matches_any_encountered_peer_directly() {
        let b = bundle(
            DtnDestination::LocalBroadcast(crate::types::BroadcastScope { radius_hops: 3 }),
            ForwardingClass::SprayAndWait,
            10_000,
            4,
        );
        let peers = vec![peer(1, false)];
        assert_eq!(
            decide_forwarding(&b, &peers, 0),
            ForwardingDecision::DeliverDirect {
                to: RouteToken(vec![1])
            }
        );
    }
}
