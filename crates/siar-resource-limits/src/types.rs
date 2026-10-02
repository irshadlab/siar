//! §3 "Resource Dimensions", §4 "Resource Classes", §8 "Resource
//! Budget".

use serde::{Deserialize, Serialize};

/// §4, verbatim variant list — §3's own instruction ("Do not collapse
/// all pressure into one generic 'busy' flag") is exactly why this is
/// ten distinct variants rather than a single `Pressure` bool or
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Memory,
    Cpu,
    Storage,
    Network,
    Connections,
    Streams,
    QueueSlots,
    FileDescriptors,
    Energy,
    Thermal,
}

/// §8, field-for-field. §8's own closing note — "Not every runtime
/// can know exact CPU/energy quantities; coarse classes are
/// acceptable" — is why [`ResourceKind::Cpu`]/[`ResourceKind::Energy`]/
/// [`ResourceKind::Thermal`] have no corresponding field here: this
/// struct only covers the dimensions §8 actually gives a concrete
/// quantity shape for (bytes, a rate, or a count). A CPU/energy/
/// thermal budget would be a coarser class-based type, not a number,
/// and isn't invented here since the spec doesn't give one.
impl ResourceKind {
    pub const COUNT: usize = 10;

    /// Declaration order is the canonical lock/acquisition order (§92-93).
    pub const ALL: [ResourceKind; Self::COUNT] = [
        Self::Memory,
        Self::Cpu,
        Self::Storage,
        Self::Network,
        Self::Connections,
        Self::Streams,
        Self::QueueSlots,
        Self::FileDescriptors,
        Self::Energy,
        Self::Thermal,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub network_bytes_per_sec: Option<u64>,
    pub max_connections: u32,
    pub max_streams: u32,
    pub max_tasks: u32,
}
