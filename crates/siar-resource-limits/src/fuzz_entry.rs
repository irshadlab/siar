//! Fuzz entry points (§184). Each function takes raw bytes, derives a
//! structured input from them, runs a security-relevant piece of this
//! crate, and `assert!`s the invariants that must hold for *every* input
//! (a panic is the finding). They are thin enough to be called directly
//! from a libFuzzer target (`fuzz/fuzz_targets/*.rs`) and are also run in
//! this module's tests over fixed edge inputs and a deterministic
//! pseudo-random corpus, so the targets themselves are covered by
//! `cargo test` and cannot silently rot.
//!
//! Not part of the supported API: `#[doc(hidden)]`.

use crate::admission::{admit, ResourceRequest};
use crate::admission_pipeline::{CookieIssuer, UnknownGateLimits, UnknownHello, UnknownPeerGate};
use crate::dynamic_config::{apply_update, ConfigUpdate, CurrentUsage};
use crate::feature_limits::FeatureLimits;
use crate::frame::FrameDecoder;
use crate::negotiation_limits::{NegotiationGate, NegotiationLimits};
use crate::policy_layers::HardLimits;
use crate::profile::{ResourceConfig, ResourceProfile};
use crate::restart::{
    plan_recovery, DurableResId, DurableReservation, OpId, PersistentResourceSettings,
    RecoveryInput, WorkEntry,
};
use crate::types::ResourceBudget;

const PROFILES: [ResourceProfile; 5] = [
    ResourceProfile::Embedded,
    ResourceProfile::MobileLow,
    ResourceProfile::MobileNormal,
    ResourceProfile::Desktop,
    ResourceProfile::Server,
];

/// Reads fixed-width integers off a byte slice; exhausted input yields 0.
struct Take<'a>(&'a [u8]);

impl Take<'_> {
    fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        let n = N.min(self.0.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        out
    }
    fn u8(&mut self) -> u8 {
        self.bytes::<1>()[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.bytes())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.bytes())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes())
    }
    fn rest(&mut self) -> &[u8] {
        std::mem::take(&mut self.0)
    }
}

#[doc(hidden)]
pub fn frame_decoder(data: &[u8]) {
    let mut t = Take(data);
    let max = usize::from(t.u16());
    let split = usize::from(t.u8()).max(1);
    let mut d = FrameDecoder::new(max);
    for piece in t.rest().chunks(split) {
        match d.feed(piece) {
            Ok(frames) => {
                for f in frames {
                    assert!(f.len() <= max, "emitted a frame over the limit");
                }
            }
            Err(_) => {
                assert!(d.is_poisoned());
                return;
            }
        }
        assert!(
            d.buffered_bytes() <= max.saturating_mul(2) + 64,
            "buffer grew beyond what the limit allows"
        );
    }
}

#[doc(hidden)]
pub fn persisted_settings(data: &[u8]) {
    let mut t = Take(data);
    let profile = PROFILES[usize::from(t.u8()) % PROFILES.len()];
    let hard = HardLimits::for_profile(profile);
    let mut cfg = ResourceConfig::for_profile(profile);
    let before = cfg;
    if let Ok(s) = postcard::from_bytes::<PersistentResourceSettings>(t.rest()) {
        match s.apply_to(&mut cfg, &hard) {
            Ok(_) => assert_eq!(
                cfg.validate(&hard),
                Ok(()),
                "applied settings left an invalid config"
            ),
            Err(_) => assert_eq!(cfg, before, "a rejected update changed the config"),
        }
    }
}

#[doc(hidden)]
pub fn quota_update(data: &[u8]) {
    let mut t = Take(data);
    let profile = PROFILES[usize::from(t.u8()) % PROFILES.len()];
    let hard = HardLimits::for_profile(profile);
    let mut cfg = ResourceConfig::for_profile(profile);
    let before = cfg;
    let flags = t.u8();
    let update = ConfigUpdate {
        relay_storage_quota: (flags & 1 != 0).then(|| t.u64()),
        max_parallel_transfers: (flags & 2 != 0).then(|| t.u32()),
        data_saver: (flags & 4 != 0).then_some(flags & 8 != 0),
    };
    let usage = CurrentUsage {
        relay_bytes: t.u64(),
        active_transfers: t.u32(),
    };
    match apply_update(&mut cfg, &hard, &update, &usage) {
        Ok(_) => assert_eq!(cfg.validate(&hard), Ok(())),
        Err(e) => {
            assert!(!e.is_empty());
            assert_eq!(cfg, before);
        }
    }
}

#[doc(hidden)]
pub fn admission_request(data: &[u8]) {
    let mut t = Take(data);
    let total = ResourceBudget {
        memory_bytes: t.u64(),
        storage_bytes: t.u64(),
        network_bytes_per_sec: None,
        max_connections: t.u32(),
        max_streams: t.u32(),
        max_tasks: 1,
    };
    let remaining = ResourceBudget {
        memory_bytes: t.u64() % total.memory_bytes.max(1),
        storage_bytes: t.u64() % total.storage_bytes.max(1),
        network_bytes_per_sec: None,
        max_connections: t.u32() % total.max_connections.max(1),
        max_streams: t.u32() % total.max_streams.max(1),
        max_tasks: 1,
    };
    if let Ok(req) = postcard::from_bytes::<ResourceRequest>(t.rest()) {
        let _ = admit(&req, &remaining, &total);
    }
}

#[doc(hidden)]
pub fn negotiation_offer(data: &[u8]) {
    let mut t = Take(data);
    let l = NegotiationLimits::default();
    let seed = t.u8();
    let mut sizes = Vec::new();
    while !t.0.is_empty() && sizes.len() < 1_024 {
        sizes.push(usize::from(t.u16()) * usize::from(t.u8()).max(1));
    }
    if NegotiationGate::new(l).begin([seed; 32], &sizes).is_ok() {
        assert!(sizes.len() <= l.max_capabilities);
        assert!(sizes.iter().all(|s| *s <= l.max_param_bytes_each));
        assert!(sizes.iter().fold(0usize, |a, b| a.saturating_add(*b)) <= l.max_param_bytes_total);
    }
}

#[doc(hidden)]
pub fn file_offer(data: &[u8]) {
    let mut t = Take(data);
    let f = FeatureLimits::for_profile(PROFILES[usize::from(t.u8()) % PROFILES.len()]).files;
    let (size, chunk) = (t.u64(), t.u64());
    if let Ok(entries) = f.check_offer(size, chunk) {
        assert!(size <= f.max_file_size && chunk <= f.max_chunk_size && chunk > 0);
        assert!(entries <= f.max_manifest_entries);
        assert_eq!(entries, size.div_ceil(chunk));
    }
}

#[doc(hidden)]
pub fn unknown_hello(data: &[u8]) {
    let mut t = Take(data);
    let under_load = t.u8() & 1 != 0;
    let issuer = CookieIssuer::new([7; 32], 10_000);
    let gate = UnknownPeerGate::new(UnknownGateLimits::default(), issuer.clone(), 0);
    let now = u64::from(t.u32());
    let cookie = (t.u8() & 1 != 0).then(|| crate::admission_pipeline::Cookie(t.bytes()));
    let payload_len = usize::from(t.u16());
    let syntax_ok = t.u8() & 1 != 0;
    let source = t.rest().to_vec();
    let hello = UnknownHello {
        source: &source,
        payload_len,
        syntax_ok,
        cookie,
    };
    let r = gate.check(&hello, now, under_load);
    if r.is_ok() && under_load {
        // admitted under load ⇒ the cookie really was valid for this source
        assert!(hello
            .cookie
            .is_some_and(|c| issuer.verify(&source, &c, now)));
    }
    assert!(
        gate.tracked_sources() <= 1,
        "a single hello cannot create more than one entry"
    );
}

#[doc(hidden)]
pub fn recovery_plan(data: &[u8]) {
    let mut t = Take(data);
    let now = u64::from(t.u16());
    let mut input = RecoveryInput::default();
    while !t.0.is_empty()
        && input.reservations.len()
            + input.work_queue.len()
            + input.staging_files.len()
            + input.active_ops.len()
            < 256
    {
        let kind = t.u8() % 4;
        let a = u64::from(t.u8() % 16);
        match kind {
            0 => input.reservations.push(DurableReservation {
                id: DurableResId(u64::from(t.u8() % 24)),
                op: (a != 0).then_some(OpId(a)),
                bytes: u64::from(t.u16()),
                owner: crate::admission::ResourceOwner::Files,
                expires_at_millis: u64::from(t.u16()),
            }),
            1 => input.work_queue.push(WorkEntry {
                op: OpId(a),
                needs_storage: t.u8() & 1 != 0,
            }),
            2 => input.staging_files.push(OpId(a)),
            _ => input.active_ops.push(OpId(a)),
        }
    }
    let p = plan_recovery(&input, now);
    let sorted = |v: &[DurableResId]| v.windows(2).all(|w| w[0] < w[1]);
    assert!(
        sorted(&p.restore) && sorted(&p.release_expired) && sorted(&p.release_orphans),
        "outputs sorted and unique"
    );
    assert!(p.delete_orphan_staging.windows(2).all(|w| w[0] < w[1]));
    // a reservation is in at most one bucket
    for id in &p.restore {
        assert!(!p.release_expired.contains(id) && !p.release_orphans.contains(id));
    }
    // every repeated id is reported
    for id in &repeated_ids(&input) {
        assert!(p.duplicate_ids.contains(id));
    }
    // restored/expired/orphaned ids all came from the input
    let ids: Vec<_> = input.reservations.iter().map(|r| r.id).collect();
    for id in p
        .restore
        .iter()
        .chain(&p.release_expired)
        .chain(&p.release_orphans)
    {
        assert!(ids.contains(id));
    }
    // staging is only deleted for operations nobody knows about
    for op in &p.delete_orphan_staging {
        assert!(!input.work_queue.iter().any(|w| w.op == *op) && !input.active_ops.contains(op));
    }
    // planning is deterministic
    assert_eq!(p, plan_recovery(&input, now));
}

fn repeated_ids(input: &RecoveryInput) -> Vec<DurableResId> {
    let mut seen = std::collections::HashSet::new();
    input
        .reservations
        .iter()
        .filter(|r| !seen.insert(r.id))
        .map(|r| r.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type Target = fn(&[u8]);
    const TARGETS: [(&str, Target); 9] = [
        ("frame_decoder", frame_decoder),
        ("persisted_settings", persisted_settings),
        ("quota_update", quota_update),
        ("admission_request", admission_request),
        ("negotiation_offer", negotiation_offer),
        ("file_offer", file_offer),
        ("unknown_hello", unknown_hello),
        ("recovery_plan", recovery_plan),
        ("noop", |_| {}),
    ];

    fn corpus() -> Vec<Vec<u8>> {
        let mut out = vec![
            vec![],
            vec![0; 1],
            vec![0; 64],
            vec![0xFF; 64],
            vec![0xFF; 4_096],
            (0..=255).collect(),
        ];
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..4_000usize {
            let len = (i * 7) % 300;
            let mut v = Vec::with_capacity(len);
            for _ in 0..len {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                v.push((x >> 32) as u8);
            }
            out.push(v);
        }
        out
    }

    #[test]
    fn every_fuzz_target_survives_edge_inputs_and_a_deterministic_corpus() {
        let corpus = corpus();
        for (name, f) in TARGETS {
            for (i, input) in corpus.iter().enumerate() {
                let r = std::panic::catch_unwind(|| f(input));
                assert!(
                    r.is_ok(),
                    "fuzz target `{name}` panicked on corpus input #{i}: {input:?}"
                );
            }
        }
    }

    #[test]
    fn take_reads_zero_past_the_end() {
        let mut t = Take(&[1, 2]);
        assert_eq!(t.u16(), 0x0201);
        assert_eq!((t.u64(), t.u8()), (0, 0));
    }
}
