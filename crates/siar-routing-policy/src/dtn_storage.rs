//! §176 "Storage Cost", §177 "Route Planning Under Storage Pressure",
//! §178 "Emergency Storage Override".
//!
//! This is a distinct kind of pressure from round 15's
//! [`crate::resource_pressure::MemoryPressure`] — that one is about
//! *this device's own* RAM; this one is about a DTN relay's finite
//! *storage* for bundles it's holding on someone else's behalf. Same
//! shape of problem, different resource, so a parallel (not shared)
//! type: [`DtnStoragePressure`].
//!
//! §178's own "evict expired/low-priority/bulk DTN items according to
//! Part 17/06 policy" is explicitly not this crate's mechanism to
//! build — the spec names the exact parts that own it. What §178
//! does ask of *this* crate is "general routing marks priority," which
//! was already true before this round:
//! [`crate::requirements::DeliveryRequirements::priority`] has existed
//! since round 1 and already flows through to every candidate this
//! crate ever plans for. Nothing new needed for that half.

use crate::candidate::PathCandidate;
use crate::descriptor::ByteCount;
use crate::requirements::DeliveryRequirements;
use crate::types::{DeliveryClass, Priority, TransportKind};

/// §176's own two named inputs, bundled — a DTN relay's storage
/// health as reported by whatever owns the actual store (Part 06),
/// the same "this crate consumes a report, it doesn't measure the
/// resource itself" shape [`crate::probability::RouteProbabilitySignal`]
/// already takes for a different DTN-specific quantity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DtnStoragePressure {
    pub relay_storage_quota_remaining: crate::metrics::Ratio,
    pub local_storage_nearly_full: bool,
}

/// §177: "if DTN store nearly full, prefer direct path, or
/// reject/defer low-priority bulk traffic." Read as two separate
/// clauses joined by "or," not one combined rule: *any* DTN candidate
/// is eliminated once storage is nearly full (forcing a direct path
/// to win on its own merits, never on DTN being artificially
/// penalized rather than removed) — [`DeliveryClass::Bulk`] at
/// [`Priority::Low`]/[`Priority::Background`] is the specific
/// "low-priority bulk" case the spec calls out for outright rejection
/// rather than just losing to a better-scoring direct path, which the
/// elimination above already handles for every other class/priority
/// combination.
pub fn eliminate_dtn_under_storage_pressure<'a>(
    candidates: &'a [PathCandidate],
    pressure: &DtnStoragePressure,
) -> Vec<&'a PathCandidate> {
    if !pressure.local_storage_nearly_full && pressure.relay_storage_quota_remaining.get() > 0.05 {
        return candidates.iter().collect();
    }
    candidates
        .iter()
        .filter(|c| c.transport != TransportKind::Dtn)
        .collect()
}

/// §177's second clause: "reject/defer low-priority bulk traffic"
/// specifically, independent of whether a DTN candidate exists at
/// all — a caller checks this before even attempting to acquire a
/// bulk transfer under storage pressure, the same early-gate shape
/// [`crate::resource_pressure::memory_pressure_allows_acquisition`]
/// already takes for device memory pressure.
pub fn storage_pressure_allows_bulk_acquisition(
    pressure: &DtnStoragePressure,
    class: DeliveryClass,
    priority: Priority,
) -> bool {
    let under_pressure =
        pressure.local_storage_nearly_full || pressure.relay_storage_quota_remaining.get() <= 0.05;
    !(under_pressure
        && class == DeliveryClass::Bulk
        && matches!(priority, Priority::Low | Priority::Background))
}

/// §178's own real half — see this module's own doc comment for why
/// the eviction mechanism itself belongs to Part 06/17, not here.
/// `estimated_size` is unused by the function itself (there's nothing
/// to compute from it once eviction isn't this crate's job) but kept
/// in the signature so a caller marking priority for a specific
/// operation has the same inputs [`DeliveryRequirements`] itself
/// would already have on hand, rather than this function silently
/// dropping a parameter a caller expected to matter.
pub fn marked_priority_for_emergency_storage(
    req: &DeliveryRequirements,
    _estimated_size: ByteCount,
) -> Priority {
    req.priority
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::metrics::{PathMetrics, Ratio};
    use crate::types::{MeteredState, PathCapabilities, PathId, RoamingState, RouteHealth};
    use siar_domain::DeviceId;

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
    fn spec_177_a_nearly_full_store_eliminates_dtn_and_leaves_direct_to_compete() {
        let candidates = vec![
            candidate(TransportKind::IrohDirect),
            candidate(TransportKind::Dtn),
        ];
        let pressure = DtnStoragePressure {
            relay_storage_quota_remaining: Ratio::new(0.01),
            local_storage_nearly_full: true,
        };
        let kept = eliminate_dtn_under_storage_pressure(&candidates, &pressure);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].transport, TransportKind::IrohDirect);
    }

    #[test]
    fn plenty_of_storage_leaves_dtn_untouched() {
        let candidates = vec![
            candidate(TransportKind::IrohDirect),
            candidate(TransportKind::Dtn),
        ];
        let pressure = DtnStoragePressure {
            relay_storage_quota_remaining: Ratio::new(0.9),
            local_storage_nearly_full: false,
        };
        let kept = eliminate_dtn_under_storage_pressure(&candidates, &pressure);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn spec_177_low_priority_bulk_is_rejected_outright_under_pressure() {
        let pressure = DtnStoragePressure {
            relay_storage_quota_remaining: Ratio::new(0.01),
            local_storage_nearly_full: true,
        };
        assert!(!storage_pressure_allows_bulk_acquisition(
            &pressure,
            DeliveryClass::Bulk,
            Priority::Low
        ));
        // Critical bulk (e.g. an SOS attachment) is not "low-priority"
        // and must not be swept up by the same rejection.
        assert!(storage_pressure_allows_bulk_acquisition(
            &pressure,
            DeliveryClass::Bulk,
            Priority::Critical
        ));
    }

    #[test]
    fn spec_178_general_routing_only_marks_priority_it_never_evicts_anything() {
        let mut req = DeliveryRequirements::emergency();
        req.priority = Priority::Critical;
        assert_eq!(
            marked_priority_for_emergency_storage(&req, ByteCount(1_000)),
            Priority::Critical
        );
    }
}
