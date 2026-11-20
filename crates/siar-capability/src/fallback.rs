//! §115 File Fallback, §116 DTN Fallback, §117 Routing Fallback.
//!
//! Each function turns a *negotiated* [`CapabilitySet`] into what the engine
//! should do with exactly those capabilities — a degraded plan rather than a
//! failure, so a peer that lacks an optional feature costs speed or deferral,
//! not the whole operation. Nothing here invents protocol semantics: it only
//! reads which capabilities were agreed, and the three spec examples are the
//! tests.
//!
//! This is the plan, not the I/O: Part 05 / Part 06 / Part 03 own the actual
//! transfer, bundle and path code and consume these plans.

use crate::descriptor::CapabilityParameters;
use crate::id::{CapabilityId, CapabilityNamespace};
use crate::set::CapabilitySet;
use crate::{dtn_extension as dtn, files_extension as files};

fn has(set: &CapabilitySet, ns: CapabilityNamespace, code: u32) -> bool {
    set.contains(&CapabilityId::new(ns, code))
}

fn u32_param(set: &CapabilitySet, ns: CapabilityNamespace, code: u32) -> Option<u32> {
    match set.get(&CapabilityId::new(ns, code))?.parameters {
        CapabilityParameters::U32(v) => Some(v),
        _ => None,
    }
}

// ------------------------------------------------------------------ §115

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStrategy {
    ParallelResumable,
    ParallelOneShot,
    /// §115's example: parallel unsupported, resume supported.
    SingleStreamResumable,
    SingleStreamOneShot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePlan {
    pub strategy: FileStrategy,
    pub partial_read: bool,
    pub multi_source: bool,
    /// The negotiated ceiling, if the peer advertised one. When `None` the file
    /// engine applies Part 05's own default; this crate does not invent a number.
    pub max_chunk_size: Option<u32>,
}

impl FilePlan {
    pub fn parallel(&self) -> bool {
        matches!(
            self.strategy,
            FileStrategy::ParallelResumable | FileStrategy::ParallelOneShot
        )
    }

    pub fn resumable(&self) -> bool {
        matches!(
            self.strategy,
            FileStrategy::ParallelResumable | FileStrategy::SingleStreamResumable
        )
    }
}

/// `None` means `files/1` is not usable with this peer at all (no fixed
/// chunking was agreed, so there is nothing to degrade *to*).
pub fn plan_file_transfer(negotiated: &CapabilitySet) -> Option<FilePlan> {
    let f = CapabilityNamespace::Files;
    if !has(negotiated, f, files::FIXED_CHUNKING) {
        return None;
    }
    let parallel = has(negotiated, f, files::PARALLEL_CHUNKS);
    let resume = has(negotiated, f, files::RESUME);
    let strategy = match (parallel, resume) {
        (true, true) => FileStrategy::ParallelResumable,
        (true, false) => FileStrategy::ParallelOneShot,
        (false, true) => FileStrategy::SingleStreamResumable,
        (false, false) => FileStrategy::SingleStreamOneShot,
    };
    Some(FilePlan {
        strategy,
        partial_read: has(negotiated, f, files::PARTIAL_READ),
        multi_source: has(negotiated, f, files::MULTI_SOURCE),
        max_chunk_size: u32_param(negotiated, f, files::MAX_CHUNK_SIZE),
    })
}

// ------------------------------------------------------------------ §116

/// What a bundle is, for the purpose of "can this peer carry it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleClass {
    /// Text, SOS, receipts: small and self-contained, inline in one bundle.
    Small,
    /// A chunk of a large blob (file transfer over DTN).
    BlobChunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Larger than the negotiated max bundle size: it cannot be sent as is.
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleDecision {
    Send,
    /// Not with this peer: keep it stored and carry it to a peer that can (DTN
    /// is store-carry-forward, so "later" is a real outcome, not a euphemism).
    Defer,
    Refuse(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtnPlan {
    pub blob_chunks: bool,
    pub gateway_handoff: bool,
    pub relay_ack: bool,
    pub spray_wait: bool,
    pub local_broadcast: bool,
    pub max_bundle_size: Option<u32>,
}

impl DtnPlan {
    /// §116's example: blob-chunk relay unsupported, small inline bundles
    /// supported → send text/SOS now, defer the large file. A small bundle is
    /// never held back for lack of blob support (an SOS must not wait on it).
    pub fn decide(&self, class: BundleClass, size_bytes: u64) -> BundleDecision {
        if class == BundleClass::BlobChunk && !self.blob_chunks {
            return BundleDecision::Defer;
        }
        match self.max_bundle_size {
            Some(max) if size_bytes > u64::from(max) => BundleDecision::Refuse(Refusal::TooLarge),
            _ => BundleDecision::Send,
        }
    }
}

/// `None` means the peer does not even do direct `dtn/1` delivery (the baseline
/// the negotiator treats as required): nothing can be handed to it.
pub fn plan_dtn(negotiated: &CapabilitySet) -> Option<DtnPlan> {
    let d = CapabilityNamespace::Dtn;
    if !has(negotiated, d, dtn::DIRECT_ONLY) {
        return None;
    }
    Some(DtnPlan {
        blob_chunks: has(negotiated, d, dtn::BLOB_CHUNK),
        gateway_handoff: has(negotiated, d, dtn::GATEWAY_HANDOFF),
        relay_ack: has(negotiated, d, dtn::RELAY_ACK),
        spray_wait: has(negotiated, d, dtn::SPRAY_WAIT),
        local_broadcast: has(negotiated, d, dtn::LOCAL_BROADCAST),
        max_bundle_size: u32_param(negotiated, d, dtn::MAX_BUNDLE_SIZE),
    })
}

// ------------------------------------------------------------------ §117

/// The usable transports in the caller's preference order. Generic over the
/// transport type because Part 03/14 own the transport vocabulary; `supported`
/// is "was this transport agreed with this peer".
///
/// §117's example: Wi-Fi Direct unsupported, Bluetooth supported → the chain
/// simply starts at Bluetooth.
pub fn fallback_chain<T: Clone>(preference: &[T], supported: impl Fn(&T) -> bool) -> Vec<T> {
    preference
        .iter()
        .filter(|t| supported(t))
        .cloned()
        .collect()
}

/// The next transport to try after some have failed: first of the chain not in
/// `failed`. The chain order is never altered, so "adapts automatically" is
/// deterministic.
pub fn next_transport<'a, T: PartialEq>(chain: &'a [T], failed: &[T]) -> Option<&'a T> {
    chain.iter().find(|t| !failed.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityRequirement};
    use crate::version::CapabilityVersion;

    fn set(caps: &[(CapabilityNamespace, u32, CapabilityParameters)]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for (ns, code, p) in caps {
            s.insert(CapabilityDescriptor::new(
                CapabilityId::new(*ns, *code),
                CapabilityVersion::new(1, 0),
                CapabilityRequirement::Optional,
                p.clone(),
            ))
            .unwrap();
        }
        s
    }

    const F: CapabilityNamespace = CapabilityNamespace::Files;
    const D: CapabilityNamespace = CapabilityNamespace::Dtn;
    const NONE: CapabilityParameters = CapabilityParameters::None;

    #[test]
    fn file_fallback_matches_the_spec_example_and_the_other_combinations() {
        let chunking = (F, files::FIXED_CHUNKING, NONE);
        let resume = (F, files::RESUME, NONE);
        let parallel = (F, files::PARALLEL_CHUNKS, NONE);
        let size = (F, files::MAX_CHUNK_SIZE, CapabilityParameters::U32(1 << 20));

        // §115: parallel unsupported, resume supported → single-stream resumable.
        let p = plan_file_transfer(&set(&[chunking.clone(), resume.clone(), size])).unwrap();
        assert_eq!(p.strategy, FileStrategy::SingleStreamResumable);
        assert!(!p.parallel() && p.resumable());
        assert_eq!(p.max_chunk_size, Some(1 << 20));

        let all = plan_file_transfer(&set(&[chunking.clone(), resume.clone(), parallel.clone()]))
            .unwrap();
        assert_eq!(all.strategy, FileStrategy::ParallelResumable);
        assert_eq!(
            all.max_chunk_size, None,
            "no advertised ceiling is reported, not invented"
        );

        let par_only = plan_file_transfer(&set(&[chunking.clone(), parallel])).unwrap();
        assert_eq!(par_only.strategy, FileStrategy::ParallelOneShot);
        let bare = plan_file_transfer(&set(&[chunking])).unwrap();
        assert_eq!(bare.strategy, FileStrategy::SingleStreamOneShot);
        assert!(!bare.parallel() && !bare.resumable());
    }

    #[test]
    fn no_chunking_means_no_file_transfer_even_if_resume_was_agreed() {
        assert_eq!(plan_file_transfer(&set(&[(F, files::RESUME, NONE)])), None);
        assert_eq!(plan_file_transfer(&CapabilitySet::new()), None);
    }

    #[test]
    fn dtn_fallback_sends_text_and_sos_but_defers_large_files() {
        // §116: blob-chunk relay unsupported, small inline bundles supported.
        let plan = plan_dtn(&set(&[
            (D, dtn::DIRECT_ONLY, NONE),
            (D, dtn::MAX_BUNDLE_SIZE, CapabilityParameters::U32(1 << 20)),
        ]))
        .unwrap();
        assert!(!plan.blob_chunks);
        assert_eq!(plan.decide(BundleClass::Small, 200), BundleDecision::Send);
        assert_eq!(
            plan.decide(BundleClass::BlobChunk, 200_000),
            BundleDecision::Defer
        );
        // Too big to be a bundle at all is refused, not deferred.
        assert_eq!(
            plan.decide(BundleClass::Small, (1 << 20) + 1),
            BundleDecision::Refuse(Refusal::TooLarge)
        );
    }

    #[test]
    fn dtn_with_blob_support_sends_chunks_within_the_size_limit() {
        let plan = plan_dtn(&set(&[
            (D, dtn::DIRECT_ONLY, NONE),
            (D, dtn::BLOB_CHUNK, NONE),
            (D, dtn::GATEWAY_HANDOFF, NONE),
            (D, dtn::MAX_BUNDLE_SIZE, CapabilityParameters::U32(4096)),
        ]))
        .unwrap();
        assert!(plan.blob_chunks && plan.gateway_handoff && !plan.relay_ack);
        assert_eq!(
            plan.decide(BundleClass::BlobChunk, 4096),
            BundleDecision::Send
        );
        assert_eq!(
            plan.decide(BundleClass::BlobChunk, 4097),
            BundleDecision::Refuse(Refusal::TooLarge)
        );
    }

    #[test]
    fn a_peer_without_direct_dtn_gets_nothing_and_an_unadvertised_limit_is_not_assumed() {
        assert_eq!(plan_dtn(&set(&[(D, dtn::BLOB_CHUNK, NONE)])), None);
        let plan = plan_dtn(&set(&[(D, dtn::DIRECT_ONLY, NONE)])).unwrap();
        assert_eq!(plan.max_bundle_size, None);
        assert_eq!(
            plan.decide(BundleClass::Small, u64::MAX),
            BundleDecision::Send
        );
    }

    #[test]
    fn routing_falls_back_down_the_callers_own_preference_order() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        enum T {
            WifiDirect,
            Bluetooth,
            Lan,
        }
        let pref = [T::WifiDirect, T::Lan, T::Bluetooth];
        // §117: Wi-Fi Direct unsupported, Bluetooth supported.
        let chain = fallback_chain(&pref, |t| *t != T::WifiDirect);
        assert_eq!(chain, [T::Lan, T::Bluetooth]);
        assert_eq!(next_transport(&chain, &[]), Some(&T::Lan));
        assert_eq!(next_transport(&chain, &[T::Lan]), Some(&T::Bluetooth));
        assert_eq!(next_transport(&chain, &[T::Lan, T::Bluetooth]), None);
        assert!(fallback_chain(&pref, |_| false).is_empty());
    }
}
