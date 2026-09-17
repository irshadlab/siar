//! Bounded, one-hop relay composition — ported from the retired
//! `siar-routing::path::{compose_via_relay, RelayAdvertisement}` (see
//! `MIGRATION.md`, step 3). Real gap this crate never had an
//! equivalent for at all: [`crate::types::TransportKind::MeshRelay`]
//! has existed as a variant since round 1, but nothing in this crate
//! has ever constructed a [`crate::candidate::PathCandidate`] carrying
//! it — every existing candidate-producing code path in this crate
//! assumes a caller already hands it a full candidate list, not that
//! this crate derives new candidates from existing ones itself.
//!
//! next.md §91's own "David → Bob/BLE" example, generalized: this
//! device reaches some peer `via` directly, and `via` has advertised
//! it can reach `destination` over its own second hop.
//! [`compose_via_relay`] derives one new [`PathCandidate`] to
//! `destination` by composing this device's own best *direct*
//! candidate to `via` with the second hop `via` itself advertised.
//!
//! Deliberately stops at one relay, not a general multi-hop search —
//! mobile mesh topology changes too quickly for an Internet-style
//! global route table to stay correct, and that reasoning applies just
//! as much to a from-scratch shortest-path search as it does to
//! caching stale entries. A caller wanting a second additional hop
//! calls this again, having first added the resulting [`PathCandidate`]
//! to whatever list it calls this module against next — one
//! deliberate, caller-driven step at a time, never an unbounded chain
//! this function walks on its own.
//!
//! **Avoiding unbounded chaining**: the original `siar-routing`
//! implementation guarded against composing through an
//! already-composed route with a runtime check on a `NextHop` field
//! [`PathCandidate`] has no equivalent of. This module instead makes
//! that guarantee true by construction: [`compose_via_relay`] only
//! ever reads from the `direct_candidates_to_via` slice a caller
//! passes in, and never reads back anything it has itself produced —
//! a caller that only ever passes candidates it independently knows
//! are direct (e.g. everything a live transport layer observed
//! first-hand) can never accidentally chain through a prior composed
//! result, because this function has no way to tell the two apart
//! even if it wanted to, and never receives its own output back as
//! input unless the caller deliberately re-feeds it one.
//!
//! No real caller exists yet — this is the computation a future
//! caller (`siar-connectivity`, per `MIGRATION.md` step 5, once it has
//! a real routing-advertisement exchange to produce
//! [`RelayAdvertisement`]s from) needs, built now so that wiring
//! doesn't also have to invent this shape from scratch later.

use crate::candidate::{PathCandidate, TransportEndpoint};
use crate::metrics::{PathMetrics, Ratio};
use crate::types::{PathId, TransportKind};
use siar_domain::DeviceId;

/// One hop `via` has advertised it can reach `destination` over — the
/// caller-supplied signal a real routing-advertisement exchange would
/// produce. This type doesn't define or send that exchange itself
/// (see this module's own top doc comment): it's the shape composition
/// needs, so that later work doesn't also have to invent this shape
/// from scratch.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayAdvertisement {
    /// The neighbor making this claim — [`compose_via_relay`]'s
    /// `direct_candidates_to_via` must contain at least one direct
    /// candidate to this device, or there's nothing to compose it
    /// with.
    pub via: DeviceId,
    pub destination: DeviceId,
    /// Opaque dial info for reaching `destination` *through* `via` —
    /// same "caller-interpreted bytes" contract
    /// [`TransportEndpoint`]'s own doc comment already states; this
    /// module never looks inside it.
    pub relay_endpoint: TransportEndpoint,
    /// The relay's own estimate of its second hop, in the same units
    /// as [`crate::metrics::PathMetrics::rtt_millis`] — what `via`
    /// itself reports about its path to `destination`, not anything
    /// this device measured directly.
    pub rtt_millis: Option<u32>,
    pub reliability: Ratio,
    pub last_seen_millis: u64,
}

/// Picks the best direct candidate to `advertisement.via` from
/// `direct_candidates_to_via` — lower `packet_loss` first (`None`
/// treated as worse than any known value, since an unmeasured path is
/// a weaker basis to compose through than a measured-good one), ties
/// broken by lower `rtt_millis`.
fn best_direct_candidate(
    direct_candidates_to_via: &[PathCandidate],
    via: DeviceId,
) -> Option<&PathCandidate> {
    direct_candidates_to_via
        .iter()
        .filter(|candidate| candidate.peer == via)
        .min_by(|a, b| {
            let loss = |c: &PathCandidate| c.metrics.packet_loss.map(Ratio::get).unwrap_or(1.0);
            loss(a).total_cmp(&loss(b)).then_with(|| {
                a.metrics
                    .rtt_millis
                    .unwrap_or(u32::MAX)
                    .cmp(&b.metrics.rtt_millis.unwrap_or(u32::MAX))
            })
        })
}

/// Derives a 2-hop [`PathCandidate`] to `advertisement.destination` by
/// composing the best direct candidate to `advertisement.via` (found
/// within `direct_candidates_to_via`) with the second hop
/// `advertisement` itself claims. Returns `None` if
/// `direct_candidates_to_via` contains no candidate for
/// `advertisement.via` at all — composing through a peer this device
/// can't itself directly reach would be a route to nowhere.
///
/// The returned candidate always carries
/// [`TransportKind::MeshRelay`] regardless of the first hop's own
/// transport — the caller's *outbound* link is still whatever reaches
/// `via`, but from a scoring perspective the resulting path is a
/// relayed one, not a direct one, and should be evaluated as such
/// (`MeshRelay`'s own place in whatever transport-preference ordering
/// a caller's scorer uses already reflects that it's a compromise
/// relative to a genuine direct path).
pub fn compose_via_relay(
    direct_candidates_to_via: &[PathCandidate],
    advertisement: &RelayAdvertisement,
) -> Option<PathCandidate> {
    let first_hop = best_direct_candidate(direct_candidates_to_via, advertisement.via)?;

    let first_hop_reliability = 1.0 - first_hop.metrics.packet_loss.map(Ratio::get).unwrap_or(0.0);
    // Two independent hops both need to succeed, so their
    // probabilities multiply — composing never produces a route more
    // reliable than either hop alone.
    let composed_reliability = first_hop_reliability * advertisement.reliability.get();
    let composed_packet_loss = Ratio::new(1.0 - composed_reliability);

    // Sums when both legs report a number; `None` (not a fabricated
    // partial sum) the moment either leg doesn't — an RTT estimate
    // missing one of its two components isn't a real estimate.
    let composed_rtt = match (first_hop.metrics.rtt_millis, advertisement.rtt_millis) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        _ => None,
    };

    let mut metrics = PathMetrics::unknown();
    metrics.rtt_millis = composed_rtt;
    metrics.packet_loss = Some(composed_packet_loss);

    Some(PathCandidate {
        path_id: PathId::new(),
        transport: TransportKind::MeshRelay,
        peer: advertisement.destination,
        endpoint: advertisement.relay_endpoint.clone(),
        metrics,
        capabilities: first_hop.capabilities,
        health: crate::link_health::health_from_reliability(composed_reliability as f32),
        underlay: None,
        state: crate::acquisition::CandidateState::Active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquisition::CandidateState;
    use crate::types::{PathCapabilities, RouteHealth};

    fn direct_candidate(
        peer: DeviceId,
        packet_loss: f64,
        rtt_millis: Option<u32>,
    ) -> PathCandidate {
        let mut metrics = PathMetrics::unknown();
        metrics.packet_loss = Some(Ratio::new(packet_loss));
        metrics.rtt_millis = rtt_millis;
        PathCandidate {
            path_id: PathId::new(),
            transport: TransportKind::LocalLan,
            peer,
            endpoint: TransportEndpoint(vec![1, 2, 3]),
            metrics,
            capabilities: PathCapabilities {
                reliable_stream: true,
                datagram: true,
                large_files: true,
                realtime_media: true,
                peer_discovery: true,
                store_and_forward: false,
                metered: crate::types::MeteredState::Unmetered,
                roaming: crate::types::RoamingState::NotRoaming,
                requires_foreground: false,
            },
            health: RouteHealth::Healthy,
            underlay: None,
            state: CandidateState::Active,
        }
    }

    fn advertisement(via: DeviceId, destination: DeviceId) -> RelayAdvertisement {
        RelayAdvertisement {
            via,
            destination,
            relay_endpoint: TransportEndpoint(vec![9, 9]),
            rtt_millis: Some(70),
            reliability: Ratio::new(0.8),
            last_seen_millis: 90,
        }
    }

    #[test]
    fn composes_rtt_sum_and_multiplies_reliability_across_both_hops() {
        let relay = DeviceId::new();
        let destination = DeviceId::new();
        let candidates = vec![direct_candidate(relay, 0.1, Some(30))];
        let composed = compose_via_relay(&candidates, &advertisement(relay, destination))
            .expect("relay has a direct candidate");
        assert_eq!(composed.transport, TransportKind::MeshRelay);
        assert_eq!(composed.peer, destination);
        assert_eq!(composed.metrics.rtt_millis, Some(100));
        // first-hop reliability 0.9 * advertised 0.8 = 0.72
        let loss = composed.metrics.packet_loss.unwrap().get();
        assert!((loss - 0.28).abs() < 1e-6);
    }

    #[test]
    fn is_none_with_no_direct_candidate_to_the_relay() {
        let relay = DeviceId::new();
        let destination = DeviceId::new();
        let unrelated = direct_candidate(DeviceId::new(), 0.0, Some(10));
        assert!(compose_via_relay(&[unrelated], &advertisement(relay, destination)).is_none());
    }

    #[test]
    fn picks_the_lowest_loss_direct_candidate_when_several_exist_for_the_same_relay() {
        let relay = DeviceId::new();
        let destination = DeviceId::new();
        let candidates = vec![
            direct_candidate(relay, 0.5, Some(500)),
            direct_candidate(relay, 0.01, Some(20)),
        ];
        let composed = compose_via_relay(&candidates, &advertisement(relay, destination))
            .expect("relay has direct candidates");
        // Composed RTT should reflect the low-loss (20ms) candidate, not the 500ms one.
        assert_eq!(composed.metrics.rtt_millis, Some(90));
    }

    #[test]
    fn missing_rtt_on_either_leg_yields_no_composed_rtt() {
        let relay = DeviceId::new();
        let destination = DeviceId::new();
        let candidates = vec![direct_candidate(relay, 0.1, None)];
        let composed = compose_via_relay(&candidates, &advertisement(relay, destination))
            .expect("relay has a direct candidate");
        assert_eq!(composed.metrics.rtt_millis, None);
    }

    #[test]
    fn composed_candidate_is_immediately_active_not_requiring_discovery() {
        let relay = DeviceId::new();
        let destination = DeviceId::new();
        let candidates = vec![direct_candidate(relay, 0.0, Some(10))];
        let composed = compose_via_relay(&candidates, &advertisement(relay, destination)).unwrap();
        assert_eq!(composed.state, CandidateState::Active);
    }
}
