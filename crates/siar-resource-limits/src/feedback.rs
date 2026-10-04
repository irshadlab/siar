//! §105 user feedback, §106 developer diagnostics, §167 UI mapping, §168
//! developer view.
//!
//! Two audiences, two vocabularies. [`UserMessage`] is a closed enum of
//! plain-language states with **no numeric payload** — a normal user can
//! never be shown a semaphore count or an internal queue code, because
//! there is nowhere in the type to put one. The localisation key is
//! stable and separate from any English text. The developer view is the
//! opposite: raw numbers, formatted consistently.

use crate::pressure::OverloadMode;
use crate::snapshot::ResourceSnapshot;
use crate::storage::PressureState;

/// Why something is not happening right now (internal vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCause {
    StorageReservationDenied,
    NoUnmeteredNetwork,
    StoragePressure(PressureState),
    MemoryPressure(PressureState),
    Mode(OverloadMode),
    BackgroundPaused,
    TransferSlotsFull,
    /// Internal bookkeeping with nothing a user can act on.
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserMessage {
    WaitingForStorage,
    WaitingForWifi,
    DeviceLowOnStorage,
    StorageNearlyFull,
    BackgroundTransferPaused,
    LargeTransfersPaused,
    LowMemoryMode,
    TooManyActiveTransfers,
}

impl UserMessage {
    /// Stable id for the UI layer's string table (translation lives
    /// there, not here).
    pub const fn l10n_key(self) -> &'static str {
        match self {
            Self::WaitingForStorage => "resource.waiting_for_storage",
            Self::WaitingForWifi => "resource.waiting_for_wifi",
            Self::DeviceLowOnStorage => "resource.device_low_on_storage",
            Self::StorageNearlyFull => "resource.storage_nearly_full",
            Self::BackgroundTransferPaused => "resource.background_transfer_paused",
            Self::LargeTransfersPaused => "resource.large_transfers_paused",
            Self::LowMemoryMode => "resource.low_memory_mode",
            Self::TooManyActiveTransfers => "resource.too_many_active_transfers",
        }
    }
}

/// `None` = show nothing (internal-only causes, or pressure too mild to
/// bother a user with).
pub fn user_message(cause: ResourceCause) -> Option<UserMessage> {
    use PressureState as P;
    match cause {
        ResourceCause::StorageReservationDenied => Some(UserMessage::WaitingForStorage),
        ResourceCause::NoUnmeteredNetwork => Some(UserMessage::WaitingForWifi),
        ResourceCause::StoragePressure(P::Normal) => None,
        ResourceCause::StoragePressure(P::Elevated) => Some(UserMessage::StorageNearlyFull),
        ResourceCause::StoragePressure(P::Critical | P::Exhausted) => {
            Some(UserMessage::DeviceLowOnStorage)
        }
        ResourceCause::MemoryPressure(P::Normal | P::Elevated) => None,
        ResourceCause::MemoryPressure(P::Critical | P::Exhausted) => {
            Some(UserMessage::LowMemoryMode)
        }
        ResourceCause::Mode(OverloadMode::Normal) => None,
        ResourceCause::Mode(OverloadMode::Shedding) => Some(UserMessage::BackgroundTransferPaused),
        ResourceCause::Mode(OverloadMode::Overload | OverloadMode::CriticalOnly) => {
            Some(UserMessage::LargeTransfersPaused)
        }
        ResourceCause::BackgroundPaused => Some(UserMessage::BackgroundTransferPaused),
        ResourceCause::TransferSlotsFull => Some(UserMessage::TooManyActiveTransfers),
        ResourceCause::Internal => None,
    }
}

// ----------------------------------------------------- developer view

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Count,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedUsage {
    pub name: String,
    pub used: u64,
    pub limit: u64,
    pub unit: Unit,
}

impl NamedUsage {
    pub fn count(name: &str, used: u64, limit: u64) -> Self {
        Self {
            name: name.to_owned(),
            used,
            limit,
            unit: Unit::Count,
        }
    }

    pub fn bytes(name: &str, used: u64, limit: u64) -> Self {
        Self {
            name: name.to_owned(),
            used,
            limit,
            unit: Unit::Bytes,
        }
    }

    fn percent(&self) -> u64 {
        if self.limit == 0 {
            return if self.used > 0 { 100 } else { 0 };
        }
        (u128::from(self.used) * 100 / u128::from(self.limit)).min(u128::from(u64::MAX)) as u64
    }

    /// §106 style: "Memory: 72/128 MiB", "Streams: 14/32".
    pub fn fraction_line(&self) -> String {
        match self.unit {
            Unit::Count => format!("{}: {}/{}", self.name, self.used, self.limit),
            Unit::Bytes => {
                let (div, suffix) = scale(self.limit);
                format!(
                    "{}: {}/{} {}",
                    self.name,
                    self.used / div,
                    self.limit / div,
                    suffix
                )
            }
        }
    }

    /// §168 style: "DTN relay 91%".
    pub fn percent_line(&self) -> String {
        format!("{} {}%", self.name, self.percent())
    }
}

fn scale(limit: u64) -> (u64, &'static str) {
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    if limit >= GIB {
        (GIB, "GiB")
    } else if limit >= MIB {
        (MIB, "MiB")
    } else if limit >= KIB {
        (KIB, "KiB")
    } else {
        (1, "B")
    }
}

fn pressure_word(p: PressureState) -> &'static str {
    match p {
        PressureState::Normal => "Normal",
        PressureState::Elevated => "Elevated",
        PressureState::Critical => "Critical",
        PressureState::Exhausted => "Exhausted",
    }
}

/// The advanced-view text block: the snapshot's own dimensions, then any
/// extra named pools the caller wants listed (queues, relay, workers),
/// then the pressure state.
pub fn developer_lines(snap: &ResourceSnapshot, extras: &[NamedUsage]) -> Vec<String> {
    let mut v = vec![
        NamedUsage::bytes("Memory", snap.memory.used, snap.memory.limit).fraction_line(),
        NamedUsage::count(
            "Streams",
            u64::from(snap.streams.used),
            u64::from(snap.streams.limit),
        )
        .fraction_line(),
        NamedUsage::count(
            "Connections",
            u64::from(snap.connections.used),
            u64::from(snap.connections.limit),
        )
        .fraction_line(),
        NamedUsage::bytes(
            "Storage",
            snap.storage.used.saturating_add(snap.storage.reserved),
            snap.storage.quota,
        )
        .fraction_line(),
    ];
    v.extend(extras.iter().map(NamedUsage::fraction_line));
    v.push(format!(
        "Pressure: {}",
        pressure_word(snap.effective_pressure())
    ));
    v
}

/// Compact percentage lines for the "what is hot" panel, hottest first.
pub fn hot_pools(pools: &[NamedUsage]) -> Vec<String> {
    let mut v: Vec<&NamedUsage> = pools.iter().collect();
    v.sort_by(|a, b| b.percent().cmp(&a.percent()).then(a.name.cmp(&b.name)));
    v.into_iter().map(NamedUsage::percent_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{CountUsage, ResourceUsage};

    #[test]
    fn user_messages_have_no_numbers_and_stable_keys() {
        let all = [
            UserMessage::WaitingForStorage,
            UserMessage::WaitingForWifi,
            UserMessage::DeviceLowOnStorage,
            UserMessage::StorageNearlyFull,
            UserMessage::BackgroundTransferPaused,
            UserMessage::LargeTransfersPaused,
            UserMessage::LowMemoryMode,
            UserMessage::TooManyActiveTransfers,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(|m| m.l10n_key()).collect();
        assert_eq!(keys.len(), all.len(), "keys are unique");
        assert!(keys
            .iter()
            .all(|k| k.starts_with("resource.") && !k.chars().any(|c| c.is_ascii_digit())));
        // closed, fieldless enum: nothing numeric can be carried
        assert_eq!(std::mem::size_of::<UserMessage>(), 1);
    }

    #[test]
    fn spec_examples_map_from_their_causes() {
        use ResourceCause::*;
        assert_eq!(
            user_message(StorageReservationDenied),
            Some(UserMessage::WaitingForStorage)
        );
        assert_eq!(
            user_message(NoUnmeteredNetwork),
            Some(UserMessage::WaitingForWifi)
        );
        assert_eq!(
            user_message(StoragePressure(PressureState::Critical)),
            Some(UserMessage::DeviceLowOnStorage)
        );
        assert_eq!(
            user_message(BackgroundPaused),
            Some(UserMessage::BackgroundTransferPaused)
        );
        assert_eq!(
            user_message(TransferSlotsFull),
            Some(UserMessage::TooManyActiveTransfers)
        );
        assert_eq!(
            user_message(MemoryPressure(PressureState::Exhausted)),
            Some(UserMessage::LowMemoryMode)
        );
        assert_eq!(
            user_message(Mode(OverloadMode::Overload)),
            Some(UserMessage::LargeTransfersPaused)
        );
    }

    #[test]
    fn mild_or_internal_causes_show_nothing() {
        use ResourceCause::*;
        assert_eq!(user_message(Internal), None);
        assert_eq!(user_message(StoragePressure(PressureState::Normal)), None);
        assert_eq!(user_message(MemoryPressure(PressureState::Elevated)), None);
        assert_eq!(user_message(Mode(OverloadMode::Normal)), None);
    }

    #[test]
    fn worsening_storage_pressure_escalates_the_message_never_regresses() {
        let msgs: Vec<_> = [
            PressureState::Normal,
            PressureState::Elevated,
            PressureState::Critical,
        ]
        .map(|p| user_message(ResourceCause::StoragePressure(p)))
        .to_vec();
        assert_eq!(
            msgs,
            vec![
                None,
                Some(UserMessage::StorageNearlyFull),
                Some(UserMessage::DeviceLowOnStorage)
            ]
        );
    }

    #[test]
    fn developer_lines_match_the_spec_format() {
        const MIB: u64 = 1 << 20;
        assert_eq!(
            NamedUsage::bytes("Memory", 72 * MIB, 128 * MIB).fraction_line(),
            "Memory: 72/128 MiB"
        );
        assert_eq!(
            NamedUsage::count("Streams", 14, 32).fraction_line(),
            "Streams: 14/32"
        );
        assert_eq!(
            NamedUsage::count("Transfers", 3, 4).fraction_line(),
            "Transfers: 3/4"
        );
        assert_eq!(
            NamedUsage::bytes("DTN relay", 420 * MIB, 512 * MIB).fraction_line(),
            "DTN relay: 420/512 MiB"
        );
        assert_eq!(
            NamedUsage::count("Bulk queue", 64, 64).percent_line(),
            "Bulk queue 100%"
        );
        assert_eq!(
            NamedUsage::bytes("Memory pool", 86, 100).percent_line(),
            "Memory pool 86%"
        );
    }

    #[test]
    fn units_scale_and_zero_limits_do_not_divide() {
        assert_eq!(
            NamedUsage::bytes("x", 3 << 30, 4 << 30).fraction_line(),
            "x: 3/4 GiB"
        );
        assert_eq!(
            NamedUsage::bytes("x", 10, 900).fraction_line(),
            "x: 10/900 B"
        );
        assert_eq!(NamedUsage::count("x", 0, 0).percent_line(), "x 0%");
        assert_eq!(NamedUsage::count("x", 1, 0).percent_line(), "x 100%");
    }

    #[test]
    fn full_developer_block() {
        const MIB: u64 = 1 << 20;
        let snap = ResourceSnapshot {
            memory: ResourceUsage::new(72 * MIB, 128 * MIB, 90 * MIB),
            streams: CountUsage {
                used: 14,
                limit: 32,
            },
            pressure: PressureState::Elevated,
            ..Default::default()
        };
        let lines = developer_lines(&snap, &[NamedUsage::count("Bulk queue", 48, 64)]);
        assert_eq!(lines[0], "Memory: 72/128 MiB");
        assert_eq!(lines[1], "Streams: 14/32");
        assert!(lines.contains(&"Bulk queue: 48/64".to_string()));
        assert_eq!(lines.last().unwrap(), "Pressure: Elevated");
    }

    #[test]
    fn hot_pools_are_sorted_hottest_first() {
        let l = hot_pools(&[
            NamedUsage::count("Peer Bob streams", 8, 8),
            NamedUsage::bytes("DTN relay", 91, 100),
            NamedUsage::count("CPU heavy workers", 4, 4),
        ]);
        assert_eq!(
            l,
            vec![
                "CPU heavy workers 100%",
                "Peer Bob streams 100%",
                "DTN relay 91%"
            ]
        );
    }
}
