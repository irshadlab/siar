//! The wire message `siar_routing_policy::relay_composition::compose_via_relay`
//! has been waiting for since it was built: that method's own doc
//! comment named its `RelayAdvertisement` parameter as "the caller-
//! supplied signal a real routing-advertisement exchange would
//! produce" and explicitly flagged that no such exchange existed
//! anywhere in this workspace. [`RouteAdvertisement`] is that exchange
//! message — a relay telling a directly-connected peer "I have a good
//! route to this destination," so the peer can compose a 2-hop
//! candidate through this relay without needing its own direct
//! connection to that destination.
//!
//! ## Why this lives in `siar-protocol`, not `siar-routing-policy`
//!
//! `RelayAdvertisement` is keyed on `iroh::
//! EndpointId` directly — fine for a pure-logic crate operating on
//! already-typed values, wrong for a wire message. `siar-protocol`
//! deliberately has no `iroh` dependency (every other wire type in this
//! crate — `MeshEnvelope::destination`, `MailboxCheckIn::device` — is
//! keyed on `siar_domain` types precisely so this crate doesn't need
//! one), so [`RouteAdvertisement::destination_endpoint`] carries an
//! `EndpointId`'s raw 32 bytes instead of the typed value itself. A
//! caller with an `iroh` dependency (`apps/emergency-node`,
//! `siar-connectivity`) reconstructs the typed `EndpointId` via
//! `iroh::EndpointId::from_bytes` — the same already-established
//! pattern `siar_messaging::PeerTicket`'s own fields use for the keys
//! it carries — before building a `siar_routing_policy::
//! relay_composition::RelayAdvertisement` from this type.
//!
//! ## What this closes, and what it still doesn't
//!
//! This is the wire format and (once a caller sends/receives it) the
//! actual exchange — `siar_routing_policy::relay_composition::compose_via_relay`'s
//! "computation built, no real source" gap is genuinely closed by
//! whichever binary wires this in. What it does NOT do on its own:
//!
//! - **No trust/verification of the claim.** Unlike `MailboxCheckIn`,
//!   there's no signature here — any peer can advertise a route to any
//!   destination, true or not. A relay accepting one is trusting its
//!   *transport-level* peer (someone it's already directly QUIC-
//!   connected to) the same way `apps/emergency-node`'s existing naive-
//!   flood forwarding already trusts whichever peer it happens to be
//!   talking to — not a new trust boundary, but also not a stronger one
//!   than that. A malicious or buggy peer can currently poison a
//!   receiver's `PathTable` with a fabricated destination/rtt/
//!   reliability; real defense against that (rate-limiting, plausibility
//!   checks, reputation) is separate follow-up work, not attempted here.
//! - **No propagation policy.** This message describes one hop's worth
//!   of advertisement; deciding *when* to send one, *to whom*, and
//!   *how often* (flooding it further would need its own loop-
//!   prevention, since nothing here caps how many times an
//!   advertisement gets re-advertised) is entirely up to whoever sends
//!   it — `apps/emergency-node`'s periodic sync loop is the one real
//!   sender this pass adds, and it deliberately advertises only its own
//!   *direct* routes, once, to its own currently-known peers — never
//!   re-advertising something it heard advertised itself, which is
//!   exactly the unbounded-flooding case this doc comment flags as not
//!   handled.

use serde::{Deserialize, Serialize};

/// One relay's claim that it has a good route to one destination — see
/// this module's top doc comment for the full picture, including what
/// this doesn't verify or prevent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RouteAdvertisement {
    /// Raw `iroh::EndpointId` bytes for the destination this
    /// advertisement claims a route to — see this module's top doc
    /// comment for why raw bytes rather than the typed value.
    pub destination_endpoint: [u8; 32],
    /// The advertiser's own estimate of its route to
    /// `destination_endpoint` — what it would report about *its* path,
    /// same "the relay's own second-hop estimate" shape
    /// `siar_routing_policy::relay_composition::RelayAdvertisement`'s own fields already
    /// have (this type exists specifically to become one of those on
    /// the receiving end).
    pub rtt_millis: Option<u32>,
    pub reliability: f32,
    /// Genuine wall-clock milliseconds, not an opaque tick — same
    /// reasoning `MailboxCheckIn::issued_at_millis`'s own doc comment
    /// gives: a receiver comparing freshness against its own `PathTable`
    /// entries (which use wall-clock-derived ticks via `siar_domain::
    /// now_millis`) needs a comparable clock, not a caller-local one.
    pub advertised_at: u64,
}
