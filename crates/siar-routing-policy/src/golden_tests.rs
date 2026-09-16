//! §183 "Testing Matrix", §184 "Route Selection Golden Tests" — the
//! spec's own five worked examples, transcribed as tests against
//! [`crate::plan::plan_route`]/[`crate::decision::decide_route`]
//! directly, each checking the exact expected outcome the spec's own
//! text names rather than merely that *a* plan comes back. §183's own
//! combinations are checked at the end of this module's test list —
//! see that section's own doc comment there for which combinations
//! already had coverage elsewhere before this round.

#[cfg(test)]
mod tests {
    use crate::acquisition::CandidateState;
    use crate::candidate::{PathCandidate, TransportEndpoint};
    use crate::decision::{
        decide_route, ApplicationPolicy, DeferredReason, PolicyLayers, RouteDecisionResult,
        SystemPolicy,
    };
    use crate::descriptor::{ByteCount, ContentClass, OperationDescriptor, OperationId};
    use crate::metrics::PathMetrics;
    use crate::plan::plan_route;
    use crate::policy::RoutingPolicyProfile;
    use crate::privacy::PrivacyPolicy;
    use crate::requirements::DeliveryRequirements;
    use crate::scoring::DefaultScorer;
    use crate::types::{
        Destination, MeteredState, PathCapabilities, PathId, RoamingState, RouteHealth,
        TransportKind,
    };
    use siar_domain::{AccountId, DeviceId};
    use siar_identity_multidevice::{
        DeviceCapabilitySet, DeviceCertificate, DeviceDirectory, DeviceDirectoryEntry,
        DeviceStatus, RootIdentityKey, TrustedAccountStore,
    };

    fn candidate(transport: TransportKind, health: RouteHealth) -> PathCandidate {
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
                metered: MeteredState::Unmetered,
                roaming: RoamingState::NotRoaming,
                requires_foreground: false,
            },
            health,
            underlay: None,
            state: CandidateState::Active,
        }
    }

    /// "Existing direct healthy + text → direct." A currently-active,
    /// still-healthy direct path must not be abandoned for a
    /// text message just because a technically-competing candidate
    /// exists — this is §34/§35's own stickiness, exercised end to
    /// end rather than at the unit level.
    #[test]
    fn spec_184_existing_direct_healthy_plus_text_stays_on_direct() {
        let direct = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let relay = candidate(TransportKind::IrohRelay, RouteHealth::Healthy);
        let candidates = vec![direct.clone(), relay];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, Some(&direct), None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::IrohDirect);
    }

    /// "Direct degraded + relay stable + text → relay." Once the
    /// current path's own health has genuinely degraded, §35's
    /// `degraded_override` means it no longer gets the stickiness
    /// benefit of the doubt.
    #[test]
    fn spec_184_direct_degraded_plus_relay_stable_plus_text_switches_to_relay() {
        let mut degraded_direct = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        degraded_direct.health = RouteHealth::Degraded;
        let current = degraded_direct.clone();
        let stable_relay = candidate(TransportKind::IrohRelay, RouteHealth::Healthy);
        let candidates = vec![degraded_direct, stable_relay];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan =
            plan_route(&candidates, &req, &policy, &scorer, Some(&current), None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::IrohRelay);
    }

    /// "Large file + unmetered LAN → LAN." Round 16's §124 tests
    /// already cover the same property with Path A/B's exact numbers
    /// — this is the spec's own §184 framing of the identical case,
    /// transcribed separately since it's named as its own golden
    /// example.
    #[test]
    fn spec_184_large_file_plus_unmetered_lan_uses_lan() {
        let mut metered_direct = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        metered_direct.capabilities.metered = MeteredState::Metered;
        let unmetered_lan = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![metered_direct, unmetered_lan];
        let req = DeliveryRequirements::file_chunk(); // allow_metered: false
        let policy = RoutingPolicyProfile::BulkTransfer.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::LocalLan);
    }

    /// "Typing + only DTN → reject/drop." `typing_indicator()`'s own
    /// `allow_dtn: false` (§57) means a DTN-only candidate set has
    /// nothing eligible at all — checked at both layers this crate
    /// offers: `plan_route` returns `Err`, and `decide_route` reports
    /// it as `Deferred`, never silently substituting some other
    /// answer.
    #[test]
    fn spec_184_typing_plus_only_dtn_is_rejected() {
        let only_dtn = vec![candidate(TransportKind::Dtn, RouteHealth::Healthy)];
        let req = DeliveryRequirements::typing_indicator();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        assert!(plan_route(&only_dtn, &req, &policy, &scorer, None, None, 0).is_err());
    }

    /// "SOS + Internet unavailable + BLE mesh → DTN/mesh." No Iroh
    /// candidate exists at all (Internet unavailable) — only a
    /// Bluetooth LE path and a mesh-relay path. `emergency()` allows
    /// every transport, so the operation must still succeed on
    /// whichever of the two wins scoring, never fail just because the
    /// Internet path is absent.
    #[test]
    fn spec_184_sos_with_no_internet_uses_ble_or_mesh() {
        let ble = candidate(TransportKind::BluetoothLe, RouteHealth::Healthy);
        let mesh = candidate(TransportKind::MeshRelay, RouteHealth::Healthy);
        let candidates = vec![ble, mesh];
        let req = DeliveryRequirements::emergency();
        let policy = RoutingPolicyProfile::Emergency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert!(matches!(
            plan.primary.transport,
            TransportKind::BluetoothLe | TransportKind::MeshRelay
        ));
    }

    /// The same "Typing + only DTN → reject/drop" example, this time
    /// through the full `decide_route` stack rather than the bare
    /// scorer — proving the rejection surfaces as a real
    /// `RouteDecisionResult`, not just a lower-level `Err`.
    #[test]
    fn spec_184_typing_plus_only_dtn_defers_through_decide_route() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let cert = DeviceCertificate::issue(
            &root,
            account,
            device,
            [1u8; 32],
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let directory = DeviceDirectory::sign(
            &root,
            account,
            1,
            vec![DeviceDirectoryEntry {
                device_id: device,
                certificate: cert,
                status: DeviceStatus::Active,
                transport_endpoints: vec![],
            }],
        );
        let mut store = TrustedAccountStore::new();
        store.accept(directory, &root.root_public_key()).unwrap();

        let mut dtn_candidate = candidate(TransportKind::Dtn, RouteHealth::Healthy);
        dtn_candidate.peer = device;
        let descriptor = OperationDescriptor {
            operation_id: OperationId::new(),
            destination: Destination::Account(account),
            requirements: DeliveryRequirements::typing_indicator(),
            estimated_size: ByteCount(10),
            content_class: ContentClass::Text,
            required_extension: None,
            created_at_millis: 0,
        };
        let layers = PolicyLayers {
            system: &SystemPolicy {
                max_operation_bytes: ByteCount(u64::MAX),
            },
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let result = decide_route(
            &[dtn_candidate],
            &descriptor,
            &layers,
            account,
            &store,
            &policy,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet)
        ));
    }

    /// §183 "Testing Matrix": most of its own named combinations
    /// already have dedicated coverage elsewhere in this crate under
    /// their own framing (direct+relay and metered-only: this
    /// module's own tests above and round 13's §125 tests;
    /// battery-saver and background-restricted: `decision.rs`'s own
    /// `battery_saver_defers_a_discovery_only_candidate_set_as_battery_policy`/
    /// `background_restriction_defers_a_foreground_only_candidate_while_backgrounded`;
    /// LAN+Internet: this module's large-file test above). Two
    /// combinations had no existing test under any framing: "BLE
    /// only" and "Wi-Fi + BLE" together.
    #[test]
    fn spec_183_ble_only_still_succeeds_for_an_ordinary_message() {
        let ble_only = vec![candidate(TransportKind::BluetoothLe, RouteHealth::Healthy)];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let plan = plan_route(&ble_only, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::BluetoothLe);
    }

    #[test]
    fn spec_183_wifi_direct_plus_ble_combination_picks_a_winner_without_error() {
        let candidates = vec![
            candidate(TransportKind::WifiDirect, RouteHealth::Healthy),
            candidate(TransportKind::BluetoothLe, RouteHealth::Healthy),
        ];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert!(matches!(
            plan.primary.transport,
            TransportKind::WifiDirect | TransportKind::BluetoothLe
        ));
    }

    /// §186 "Fuzzing": this crate has no `cargo-fuzz` harness (a real,
    /// named gap — see `lib.rs`'s own Definition of Done section).
    /// What these two tests prove instead is that a specific edge
    /// case a fuzzer would eventually find — `max_latency_millis:
    /// Some(0)` paired with `rtt_millis: Some(0)`, which produces a
    /// `0.0 / 0.0` (`NaN`) inside `DefaultScorer::score`'s own
    /// latency term — doesn't panic. `plan_route`'s own sort
    /// comparator already guards this
    /// (`.partial_cmp(...).unwrap_or(Ordering::Equal)`, written for
    /// §123 "Deterministic Scoring") — this test exercises that guard
    /// against the actual malformed input, rather than asserting it
    /// exists.
    #[test]
    fn spec_186_a_zero_latency_deadline_producing_nan_does_not_panic() {
        let mut req = DeliveryRequirements::interactive_message();
        req.max_latency_millis = Some(0);
        let mut zero_rtt = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        zero_rtt.metrics.rtt_millis = Some(0);
        let normal = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![zero_rtt, normal];
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        // The point of this test is that this call returns at all.
        let result = plan_route(&candidates, &req, &policy, &scorer, None, None, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn spec_186_an_operation_estimated_at_u64_max_bytes_does_not_panic() {
        let (store, account, _device) = trusted_store_with_one_device_for_golden_tests();
        let descriptor = OperationDescriptor {
            operation_id: OperationId::new(),
            destination: Destination::Account(account),
            requirements: DeliveryRequirements::file_chunk(),
            estimated_size: ByteCount(u64::MAX),
            content_class: ContentClass::File,
            required_extension: None,
            created_at_millis: 0,
        };
        let layers = PolicyLayers {
            system: &SystemPolicy {
                max_operation_bytes: ByteCount(1_000_000), // far below u64::MAX, on purpose
            },
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let candidates = vec![candidate(TransportKind::LocalLan, RouteHealth::Healthy)];

        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &policy,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Rejected(crate::decision::RejectReason::ExceedsHardSizeLimit)
        ));
    }

    fn trusted_store_with_one_device_for_golden_tests() -> (TrustedAccountStore, AccountId, DeviceId)
    {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let cert = DeviceCertificate::issue(
            &root,
            account,
            device,
            [1u8; 32],
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let directory = DeviceDirectory::sign(
            &root,
            account,
            1,
            vec![DeviceDirectoryEntry {
                device_id: device,
                certificate: cert,
                status: DeviceStatus::Active,
                transport_endpoints: vec![],
            }],
        );
        let mut store = TrustedAccountStore::new();
        store.accept(directory, &root.root_public_key()).unwrap();
        (store, account, device)
    }
}
