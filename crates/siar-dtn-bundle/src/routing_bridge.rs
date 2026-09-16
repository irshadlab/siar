//! Closes this crate's own long-standing gap ("no routing integration
//! with siar-routing-policy" — per [[resilient-mesh]] project memory)
//! and gives real substance to §23's pipeline this crate's `lib.rs`
//! already names but doesn't implement: "persist operation → select
//! DTN bundle policy → wait for peer encounter → forward
//! opportunistically." [`crate::forwarding::decide_forwarding`] is the
//! "forward opportunistically" step; this module is the "select DTN
//! bundle policy" step — turning a
//! `siar_routing_policy::requirements::DeliveryRequirements` (an
//! application's stated delivery intent) into the concrete
//! [`crate::types::DtnPriority`]/[`crate::types::ForwardingClass`]
//! pair a [`crate::bundle::DtnBundle`] actually needs.
//!
//! Neither spec text names this bridge directly — Part 06's own §23
//! names the pipeline step without specifying how it maps from Part
//! 03's requirements type (Part 03 didn't exist as a concrete crate
//! when Part 06 was written), so the mapping below is this module's
//! own reasoned policy, documented inline rather than presented as a
//! transcription.

use crate::types::{DtnPriority, ForwardingClass};
use siar_routing_policy::requirements::DeliveryRequirements;
use siar_routing_policy::types::{DeliveryClass, Priority};

/// The output of "select DTN bundle policy" — everything
/// [`crate::bundle::DtnBundle`] needs from routing besides the
/// payload/destination/source it already gets from elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtnBundlePolicy {
    pub priority: DtnPriority,
    pub forwarding_class: ForwardingClass,
    pub expires_at_millis: u64,
}

/// §20's default bundle TTL isn't given a concrete number anywhere in
/// Part 06's spec text (only that bundles *have* an expiry) — 24
/// hours is this module's own reasonable default for a
/// `DeliveryRequirements` that doesn't specify `expiry_millis`,
/// chosen as "long enough for real store-carry-forward to matter,
/// short enough not to accumulate unbounded stale bundles," not a
/// transcribed spec value.
pub const DEFAULT_BUNDLE_TTL_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// Derives a [`DtnBundlePolicy`] from `req`, or `None` if `req` itself
/// rules DTN out (`allow_dtn: false` — §29-style policy profiles like
/// [`DeliveryRequirements::realtime_media`] set this for exactly the
/// reason [[resilient-mesh]] project memory already records: "a
/// realtime frame that arrives after a DTN hop is arriving too late to
/// be useful at all"). A caller should treat `None` as "don't create a
/// DTN bundle for this operation," not as an error to retry.
pub fn select_dtn_bundle_policy(
    req: &DeliveryRequirements,
    now_millis: u64,
) -> Option<DtnBundlePolicy> {
    if !req.allow_dtn {
        return None;
    }

    let priority = match req.priority {
        Priority::Critical => DtnPriority::Sos,
        Priority::High => DtnPriority::Important,
        Priority::Normal => DtnPriority::Normal,
        // §5's `Priority` scale has no DTN-side `Background` tier to
        // land on (`DtnPriority` only goes as low as `Low`) — both
        // collapse onto the same lowest tier rather than this module
        // inventing a fifth `DtnPriority` variant the spec never asks
        // for.
        Priority::Low | Priority::Background => DtnPriority::Low,
    };

    // `allow_relay: false` is this bundle's hard instruction not to
    // pass through any intermediary — the same meaning `ForwardingClass::
    // DirectOnly` already has in `forwarding.rs`, so it wins outright
    // regardless of `class`. Otherwise, `DeliveryClass::DelayTolerant`
    // traffic is exactly what spray-and-wait's broader, patient
    // dissemination suits (§23's own framing); anything else that
    // still allows relaying is better served trying for a fast path
    // back to infrastructure first (§26), falling back to spraying
    // only when `forwarding.rs`'s own `decide_forwarding` finds no
    // gateway present — a choice already implemented there, not
    // duplicated here.
    let forwarding_class = if !req.allow_relay {
        ForwardingClass::DirectOnly
    } else if req.class == DeliveryClass::DelayTolerant {
        ForwardingClass::SprayAndWait
    } else {
        ForwardingClass::GatewayPreferred
    };

    let ttl = req.expiry_millis.unwrap_or(DEFAULT_BUNDLE_TTL_MILLIS);

    Some(DtnBundlePolicy {
        priority,
        forwarding_class,
        expires_at_millis: now_millis.saturating_add(ttl),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_requirements() -> DeliveryRequirements {
        DeliveryRequirements {
            class: DeliveryClass::Reliable,
            priority: Priority::Normal,
            max_latency_millis: None,
            min_bandwidth: None,
            durable: true,
            allow_metered: true,
            allow_relay: true,
            allow_bluetooth: true,
            allow_dtn: true,
            allow_multipath: false,
            expiry_millis: None,
            max_cost: None,
            // Added to `DeliveryRequirements` by `siar-routing-policy`
            // for its own §52/§56 work — irrelevant to this bridge's
            // forwarding-class/TTL derivation, so left at their
            // no-op defaults rather than exercised by these tests.
            nearby_session_explicit: false,
            dtn_replication_budget: None,
            // Same reasoning as the two fields just above: irrelevant
            // to this bridge's forwarding-class/TTL derivation.
            allow_roaming_bulk: true,
            // Added to `DeliveryRequirements` by `siar-routing-policy`
            // for its own §162 work — likewise irrelevant here.
            allow_redundancy: true,
        }
    }

    #[test]
    fn dtn_disallowed_by_requirements_yields_no_policy() {
        let mut req = base_requirements();
        req.allow_dtn = false;
        assert_eq!(select_dtn_bundle_policy(&req, 0), None);
    }

    #[test]
    fn realtime_media_requirements_never_produce_a_dtn_policy() {
        // Ties directly to `DeliveryRequirements::realtime_media()`'s
        // own worked example (`allow_dtn: false`) rather than
        // reconstructing the same fields by hand.
        let req = DeliveryRequirements::realtime_media();
        assert_eq!(select_dtn_bundle_policy(&req, 0), None);
    }

    #[test]
    fn critical_priority_maps_to_sos_with_its_own_worked_replication_budget() {
        let mut req = base_requirements();
        req.priority = Priority::Critical;
        let policy = select_dtn_bundle_policy(&req, 0).unwrap();
        assert_eq!(policy.priority, DtnPriority::Sos);
        // §22's own worked example ("SOS = 8"), asserted via the
        // existing method rather than a duplicated magic number.
        assert_eq!(policy.priority.default_replication_budget(), 8);
    }

    #[test]
    fn disallowing_relay_forces_direct_only_regardless_of_class() {
        let mut req = base_requirements();
        req.allow_relay = false;
        req.class = DeliveryClass::DelayTolerant;
        let policy = select_dtn_bundle_policy(&req, 0).unwrap();
        assert_eq!(policy.forwarding_class, ForwardingClass::DirectOnly);
    }

    #[test]
    fn delay_tolerant_class_with_relay_allowed_prefers_spray_and_wait() {
        let mut req = base_requirements();
        req.class = DeliveryClass::DelayTolerant;
        let policy = select_dtn_bundle_policy(&req, 0).unwrap();
        assert_eq!(policy.forwarding_class, ForwardingClass::SprayAndWait);
    }

    #[test]
    fn reliable_class_with_relay_allowed_prefers_gateway() {
        let req = base_requirements(); // class: Reliable, allow_relay: true
        let policy = select_dtn_bundle_policy(&req, 0).unwrap();
        assert_eq!(policy.forwarding_class, ForwardingClass::GatewayPreferred);
    }

    #[test]
    fn missing_expiry_falls_back_to_the_default_ttl() {
        let req = base_requirements();
        let policy = select_dtn_bundle_policy(&req, 1_000).unwrap();
        assert_eq!(policy.expires_at_millis, 1_000 + DEFAULT_BUNDLE_TTL_MILLIS);
    }

    #[test]
    fn explicit_expiry_is_honored_over_the_default() {
        let mut req = base_requirements();
        req.expiry_millis = Some(60_000);
        let policy = select_dtn_bundle_policy(&req, 1_000).unwrap();
        assert_eq!(policy.expires_at_millis, 61_000);
    }
}
