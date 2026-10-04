//! §184 fuzz targets, run in-tree with proptest (random bytes / values,
//! shrinking on failure) so they execute in every `cargo test`. The spec's
//! four targets: resource config parser, quota update messages,
//! admission request decoding, limit negotiation — plus the frame
//! decoder, which is the first thing untrusted bytes reach.
//!
//! Property for all of them: **no input panics or overflows**, and any
//! input that is accepted leaves the system in a valid state.

use proptest::prelude::*;

use siar_resource_limits::admission::{admit, ResourceRequest};
use siar_resource_limits::dynamic_config::{apply_update, ConfigUpdate, CurrentUsage};
use siar_resource_limits::effective_limits::{effective_limit, RuntimeLimit};
use siar_resource_limits::feature_limits::FeatureLimits;
use siar_resource_limits::frame::FrameDecoder;
use siar_resource_limits::negotiation_limits::{NegotiationGate, NegotiationLimits};
use siar_resource_limits::policy_layers::HardLimits;
use siar_resource_limits::profile::{ResourceConfig, ResourceProfile};
use siar_resource_limits::restart::PersistentResourceSettings;
use siar_resource_limits::safety::{declared_len, DecompressionGuard};
use siar_resource_limits::storage::PressureState;
use siar_resource_limits::types::ResourceBudget;

const PROFILES: [ResourceProfile; 5] = [
    ResourceProfile::Embedded,
    ResourceProfile::MobileLow,
    ResourceProfile::MobileNormal,
    ResourceProfile::Desktop,
    ResourceProfile::Server,
];

proptest! {
    /// Frame decoder: arbitrary bytes in arbitrary read sizes.
    #[test]
    fn frame_decoder_never_panics_and_never_emits_an_oversize_frame(
        bytes in prop::collection::vec(any::<u8>(), 0..4_000),
        split in 1usize..300,
        max in 0usize..512,
    ) {
        let mut d = FrameDecoder::new(max);
        for piece in bytes.chunks(split) {
            match d.feed(piece) {
                Ok(frames) => for f in frames { prop_assert!(f.len() <= max); },
                Err(_) => { prop_assert!(d.is_poisoned()); break; }
            }
            prop_assert!(d.buffered_bytes() <= max.max(64) * 2 + 64, "buffer grew past what was received/allowed");
        }
    }

    /// "resource config parser": persisted settings decoded from arbitrary
    /// bytes. Whatever decodes either applies cleanly to a *valid* config
    /// or is rejected leaving the config untouched.
    #[test]
    fn settings_decoding_and_application_is_safe(bytes in prop::collection::vec(any::<u8>(), 0..64), p in 0usize..5) {
        let profile = PROFILES[p];
        let hard = HardLimits::for_profile(profile);
        let mut cfg = ResourceConfig::for_profile(profile);
        let before = cfg;
        if let Ok(s) = postcard::from_bytes::<PersistentResourceSettings>(&bytes) {
            match s.apply_to(&mut cfg, &hard) {
                Ok(_) => prop_assert_eq!(cfg.validate(&hard), Ok(())),
                Err(_) => prop_assert_eq!(cfg, before),
            }
        }
    }

    /// "quota update messages": arbitrary updates to a live config.
    #[test]
    fn quota_updates_are_atomic_and_never_break_validity(
        p in 0usize..5,
        relay in prop::option::of(any::<u64>()),
        xfers in prop::option::of(any::<u32>()),
        saver in prop::option::of(any::<bool>()),
        usage_relay in any::<u64>(), usage_xfers in any::<u32>(),
    ) {
        let profile = PROFILES[p];
        let hard = HardLimits::for_profile(profile);
        let mut cfg = ResourceConfig::for_profile(profile);
        let before = cfg;
        let r = apply_update(
            &mut cfg, &hard,
            &ConfigUpdate { relay_storage_quota: relay, max_parallel_transfers: xfers, data_saver: saver },
            &CurrentUsage { relay_bytes: usage_relay, active_transfers: usage_xfers },
        );
        match r {
            Ok(rep) => {
                prop_assert_eq!(cfg.validate(&hard), Ok(()));
                prop_assert!(rep.transfers_to_drain <= usage_xfers);
            }
            Err(errs) => { prop_assert!(!errs.is_empty()); prop_assert_eq!(cfg, before); }
        }
    }

    /// "admission request decoding": arbitrary bytes → ResourceRequest →
    /// admit(), with arbitrary budgets.
    #[test]
    fn admission_request_decoding_and_decision_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..96),
        mem in any::<u64>(), sto in any::<u64>(), conns in any::<u32>(), streams in any::<u32>(),
    ) {
        if let Ok(req) = postcard::from_bytes::<ResourceRequest>(&bytes) {
            let total = ResourceBudget { memory_bytes: mem, storage_bytes: sto, network_bytes_per_sec: None, max_connections: conns, max_streams: streams, max_tasks: 1 };
            let remaining = ResourceBudget { memory_bytes: mem / 2, storage_bytes: sto / 2, network_bytes_per_sec: None, max_connections: conns / 2, max_streams: streams / 2, max_tasks: 1 };
            let _ = admit(&req, &remaining, &total);
        }
    }

    /// "limit negotiation": the effective limit is never above any input.
    #[test]
    fn effective_limit_is_the_minimum_of_every_input(
        hard in any::<u64>(), policy in any::<u64>(),
        neg in prop::option::of(any::<u64>()), pr in prop::option::of(any::<u64>()),
    ) {
        let e = effective_limit(hard, policy, neg, pr);
        prop_assert!(e.value <= hard && e.value <= policy);
        if let Some(n) = neg { prop_assert!(e.value <= n); }
        if let Some(p) = pr { prop_assert!(e.value <= p); }
        prop_assert!(e.value == hard || e.value == policy || Some(e.value) == neg || Some(e.value) == pr);
    }

    #[test]
    fn runtime_limit_never_exceeds_hard_or_policy_whatever_the_remote_negotiates(
        hard in any::<u64>(), policy in any::<u64>(), neg in any::<u64>(), pressure in 0usize..4, dep in any::<bool>(),
    ) {
        let mut l = RuntimeLimit::new(hard, policy, dep);
        l.set_negotiated(neg);
        l.set_pressure([PressureState::Normal, PressureState::Elevated, PressureState::Critical, PressureState::Exhausted][pressure]);
        let e = l.effective();
        prop_assert!(e.value <= hard && e.value <= policy && e.value <= neg);
        // a nonzero effective cap stays nonzero under throttling
        if hard > 0 && policy > 0 && neg > 0 { prop_assert!(e.value >= 1); }
    }

    #[test]
    fn negotiation_offers_with_arbitrary_shapes_never_panic_and_obey_limits(
        sizes in prop::collection::vec(0usize..100_000, 0..600), seed in any::<u8>(),
    ) {
        let l = NegotiationLimits::default();
        let g = NegotiationGate::new(l);
        let r = g.begin([seed; 32], &sizes);
        if r.is_ok() {
            prop_assert!(sizes.len() <= l.max_capabilities);
            prop_assert!(sizes.iter().all(|s| *s <= l.max_param_bytes_each));
            prop_assert!(sizes.iter().sum::<usize>() <= l.max_param_bytes_total);
        }
    }

    #[test]
    fn file_offers_with_arbitrary_numbers_are_checked_without_overflow(file in any::<u64>(), chunk in any::<u64>(), p in 0usize..5) {
        let f = FeatureLimits::for_profile(PROFILES[p]).files;
        if let Ok(entries) = f.check_offer(file, chunk) {
            prop_assert!(file <= f.max_file_size && chunk <= f.max_chunk_size && entries <= f.max_manifest_entries);
            prop_assert!(chunk > 0);
        }
    }

    #[test]
    fn length_and_decompression_guards_hold_for_any_numbers(
        declared in any::<u64>(), limit in any::<usize>(),
        steps in prop::collection::vec((any::<u64>(), any::<u64>()), 0..20),
        max_out in any::<u64>(), ratio in any::<u64>(), grace in any::<u64>(),
    ) {
        if let Ok(n) = declared_len(declared, limit) { prop_assert!(n <= limit && n as u64 == declared); }
        let mut g = DecompressionGuard::new(max_out, ratio, grace);
        let mut produced_total = 0u64;
        for (c, p) in steps {
            produced_total = produced_total.saturating_add(p);
            if g.record(c, p).is_ok() { prop_assert!(produced_total <= max_out); } else { break; }
        }
    }
}
