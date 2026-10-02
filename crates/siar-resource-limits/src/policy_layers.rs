//! §5 policy layers, §6 hard safety limits, §7 soft limits.
//!
//! Layers can only *tighten*: the effective value is the minimum of the
//! hard ceiling and every layer that set one. A lower layer therefore
//! cannot loosen what a higher layer restricted, nor exceed the ceiling,
//! whatever order the layers are applied in.

use crate::types::ResourceKind;

/// §5, top to bottom (System hard limits are the ceiling itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PolicyLayer {
    Runtime,
    Application,
    Extension,
    Peer,
    Operation,
}

impl PolicyLayer {
    const COUNT: usize = 5;
    const ALL: [PolicyLayer; Self::COUNT] = [
        Self::Runtime,
        Self::Application,
        Self::Extension,
        Self::Peer,
        Self::Operation,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    Applied,
    /// The requested value was above the hard ceiling and was clamped.
    ClampedToHard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayeredLimit {
    hard: u64,
    layers: [Option<u64>; PolicyLayer::COUNT],
}

impl LayeredLimit {
    pub fn new(hard: u64) -> Self {
        Self {
            hard,
            layers: [None; PolicyLayer::COUNT],
        }
    }

    pub fn set(&mut self, layer: PolicyLayer, value: u64) -> SetOutcome {
        let (v, outcome) = if value > self.hard {
            (self.hard, SetOutcome::ClampedToHard)
        } else {
            (value, SetOutcome::Applied)
        };
        self.layers[layer as usize] = Some(v);
        outcome
    }

    pub fn clear(&mut self, layer: PolicyLayer) {
        self.layers[layer as usize] = None;
    }

    pub fn effective(&self) -> u64 {
        self.layers
            .iter()
            .flatten()
            .copied()
            .fold(self.hard, u64::min)
    }

    /// Which layer is currently the binding constraint (`None` = the hard
    /// ceiling itself). Ties go to the highest layer — the one that
    /// "owns" the restriction. Useful for explaining a rejection (§104).
    pub fn binding_layer(&self) -> Option<PolicyLayer> {
        let eff = self.effective();
        PolicyLayer::ALL
            .into_iter()
            .find(|l| self.layers[*l as usize] == Some(eff))
    }
}

/// §6: correctness/availability ceilings. Not user preferences — nothing
/// configurable may exceed these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardLimits {
    pub max_frame_bytes: u64,
    pub max_event_bytes: u64,
    pub max_manifest_bytes: u64,
    pub max_concurrent_streams: u64,
    pub max_memory_buffer_bytes: u64,
    pub max_dtn_storage_bytes: u64,
    pub max_pending_outbox_ops: u64,
}

impl HardLimits {
    /// The ceiling for one [`ResourceKind`], where the spec's list has one.
    pub fn for_kind(&self, kind: ResourceKind) -> Option<u64> {
        match kind {
            ResourceKind::Memory => Some(self.max_memory_buffer_bytes),
            ResourceKind::Streams => Some(self.max_concurrent_streams),
            ResourceKind::Storage => Some(self.max_dtn_storage_bytes),
            _ => None,
        }
    }
}

/// §7: ordered cheapest-to-most-disruptive adaptations to try *before*
/// hard rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adaptation {
    ReduceParallelism,
    PauseBackgroundTransfers,
    LowerVideoBitrate,
    DeferSync,
    EvictCache,
}

pub const ADAPTATION_LADDER: [Adaptation; 5] = [
    Adaptation::ReduceParallelism,
    Adaptation::PauseBackgroundTransfers,
    Adaptation::LowerVideoBitrate,
    Adaptation::DeferSync,
    Adaptation::EvictCache,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftOutcome {
    Ok,
    /// Apply these (cumulative, in order); the deeper into the soft→hard
    /// band, the more of the ladder.
    Adapt(&'static [Adaptation]),
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("soft limit {soft} exceeds hard limit {hard}")]
pub struct SoftAboveHard {
    pub soft: u64,
    pub hard: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoftLimit {
    soft: u64,
    hard: u64,
}

impl SoftLimit {
    pub fn new(soft: u64, hard: u64) -> Result<Self, SoftAboveHard> {
        if soft > hard {
            return Err(SoftAboveHard { soft, hard });
        }
        Ok(Self { soft, hard })
    }

    pub fn evaluate(&self, used: u64) -> SoftOutcome {
        if used <= self.soft {
            return SoftOutcome::Ok;
        }
        if used >= self.hard {
            return SoftOutcome::Reject;
        }
        let band = u128::from(self.hard - self.soft);
        let into = u128::from(used - self.soft);
        let n = ADAPTATION_LADDER.len() as u128;
        // ceil(into * n / band), at least 1 since used > soft
        let steps = ((into * n).div_ceil(band)).clamp(1, n) as usize;
        SoftOutcome::Adapt(&ADAPTATION_LADDER[..steps])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_only_tighten_and_order_does_not_matter() {
        let mut a = LayeredLimit::new(100);
        a.set(PolicyLayer::Runtime, 80);
        a.set(PolicyLayer::Peer, 30);
        a.set(PolicyLayer::Operation, 90); // tries to loosen: no effect
        let mut b = LayeredLimit::new(100);
        b.set(PolicyLayer::Operation, 90);
        b.set(PolicyLayer::Peer, 30);
        b.set(PolicyLayer::Runtime, 80);
        assert_eq!(a.effective(), 30);
        assert_eq!(b.effective(), 30);
    }

    #[test]
    fn nothing_exceeds_the_hard_ceiling() {
        let mut l = LayeredLimit::new(100);
        assert_eq!(
            l.set(PolicyLayer::Application, 10_000),
            SetOutcome::ClampedToHard
        );
        assert_eq!(l.effective(), 100);
        assert_eq!(l.binding_layer(), Some(PolicyLayer::Application));
        l.clear(PolicyLayer::Application);
        assert_eq!(l.binding_layer(), None);
    }

    #[test]
    fn binding_layer_reports_the_highest_owner_on_ties() {
        let mut l = LayeredLimit::new(100);
        l.set(PolicyLayer::Extension, 40);
        l.set(PolicyLayer::Runtime, 40);
        assert_eq!(l.binding_layer(), Some(PolicyLayer::Runtime));
    }

    #[test]
    fn soft_band_scales_adaptation_with_depth() {
        let s = SoftLimit::new(100, 200).unwrap();
        assert_eq!(s.evaluate(100), SoftOutcome::Ok);
        assert_eq!(s.evaluate(101), SoftOutcome::Adapt(&ADAPTATION_LADDER[..1]));
        assert_eq!(s.evaluate(150), SoftOutcome::Adapt(&ADAPTATION_LADDER[..3]));
        assert_eq!(s.evaluate(199), SoftOutcome::Adapt(&ADAPTATION_LADDER[..5]));
        assert_eq!(s.evaluate(200), SoftOutcome::Reject);
    }

    #[test]
    fn degenerate_soft_limits() {
        assert!(SoftLimit::new(5, 4).is_err());
        let same = SoftLimit::new(10, 10).unwrap();
        assert_eq!(same.evaluate(10), SoftOutcome::Ok);
        assert_eq!(same.evaluate(11), SoftOutcome::Reject);
        let huge = SoftLimit::new(0, u64::MAX).unwrap();
        assert!(matches!(huge.evaluate(u64::MAX - 1), SoftOutcome::Adapt(_)));
    }

    #[test]
    fn hard_limits_map_to_kinds() {
        let h = HardLimits {
            max_frame_bytes: 1,
            max_event_bytes: 2,
            max_manifest_bytes: 3,
            max_concurrent_streams: 4,
            max_memory_buffer_bytes: 5,
            max_dtn_storage_bytes: 6,
            max_pending_outbox_ops: 7,
        };
        assert_eq!(h.for_kind(ResourceKind::Streams), Some(4));
        assert_eq!(h.for_kind(ResourceKind::Cpu), None);
    }
}
