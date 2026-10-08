//! §167 "Versioned Recovery Protocol", §168 "Rolling Upgrade", §169
//! "Recovery and Relay Cluster", §176 "Recovery and Diagnostics",
//! §177 "Recovery and FFI", §178 "Recovery and WASM".
//!
//! The seams where recovery meets other processes, versions and
//! runtimes. In every case the durable core stays the source of
//! truth, and a peer on the other side of a seam is never allowed to
//! *assume* a capability it hasn't negotiated.

use crate::platform_recovery::IpcClient;
use crate::reporting::{RecoveryAction, RecoveryActionKind, RecoveryReport};
use crate::runtime_api::{HealthState, RuntimeHealth};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §167
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RecoveryFeature {
    Report = 0,
    Health = 1,
    Snapshot = 2,
    Admission = 3,
    Diagnostics = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FeatureSet(u8);

impl FeatureSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn with(self, f: RecoveryFeature) -> Self {
        Self(self.0 | (1 << f as u8))
    }

    pub const fn contains(self, f: RecoveryFeature) -> bool {
        self.0 & (1 << f as u8) != 0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolHello {
    pub major: u16,
    pub minor: u16,
    pub features: FeatureSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("incompatible recovery protocol: client major {client}, daemon major {daemon}")]
pub struct MajorMismatch {
    pub client: u16,
    pub daemon: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub major: u16,
    pub minor: u16,
    pub features: FeatureSet,
}

impl Negotiated {
    /// §167: the GUI may rely on recovered state *only* if the
    /// negotiation said both sides have it.
    pub const fn supports(&self, f: RecoveryFeature) -> bool {
        self.features.contains(f)
    }
}

/// Same major version required; the minor is the lower of the two; the
/// usable features are the intersection.
pub const fn negotiate(
    client: ProtocolHello,
    daemon: ProtocolHello,
) -> Result<Negotiated, MajorMismatch> {
    if client.major != daemon.major {
        return Err(MajorMismatch {
            client: client.major,
            daemon: daemon.major,
        });
    }
    Ok(Negotiated {
        major: client.major,
        minor: if client.minor < daemon.minor {
            client.minor
        } else {
            daemon.minor
        },
        features: client.features.intersect(daemon.features),
    })
}

// ---------------------------------------------------------------------
// §168
// ---------------------------------------------------------------------

/// Persisted wire/session format support of one node.
///
/// A node must be able to *read ahead*: during a rollout the
/// not-yet-upgraded nodes see state written by upgraded ones. So a
/// release that wants to start writing format N+1 must first ship in a
/// release that already **reads** N+1 while still **writing** N.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSupport {
    /// The format version this node writes.
    pub write_version: u32,
    /// The oldest format version this node can still read.
    pub min_read_version: u32,
    /// The newest format version this node can read.
    pub max_read_version: u32,
}

pub const fn can_read(reader: FormatSupport, written: u32) -> bool {
    written >= reader.min_read_version && written <= reader.max_read_version
}

pub const fn mutually_compatible(a: FormatSupport, b: FormatSupport) -> bool {
    can_read(a, b.write_version) && can_read(b, a.write_version)
}

/// §168: a cluster may be restarted node by node only if every pair of
/// nodes present during the rollout can read each other's persisted
/// state — no simultaneous upgrade required.
pub fn rolling_upgrade_ok(nodes: &[FormatSupport]) -> bool {
    nodes
        .iter()
        .enumerate()
        .all(|(i, a)| nodes[i + 1..].iter().all(|b| mutually_compatible(*a, *b)))
}

// ---------------------------------------------------------------------
// §169
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RelayDurableKind {
    /// Ciphertext queued for an offline recipient.
    EncryptedQueue,
    /// Session/routing metadata the relay needs to function.
    SessionMetadata,
    /// Anything that would let the relay read user messages.
    MessagePlaintext,
    /// User E2EE key material.
    UserE2eeKeys,
}

/// §169: user-message E2EE state stays client-side. A relay restores
/// its queues and session metadata as designed — and never persists
/// plaintext or user keys, so its recovery cannot depend on them.
pub const fn relay_may_persist(k: RelayDurableKind) -> bool {
    matches!(
        k,
        RelayDurableKind::EncryptedQueue | RelayDurableKind::SessionMetadata
    )
}

// ---------------------------------------------------------------------
// §176
// ---------------------------------------------------------------------

/// The three questions §176 says diagnostics should answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsView {
    /// What was recovered (repaired / rebuilt / cleaned).
    pub recovered: Vec<RecoveryAction>,
    /// What operation resumed.
    pub resumed: Vec<RecoveryAction>,
    /// Work that was paused and is waiting.
    pub paused: Vec<RecoveryAction>,
    /// What remains degraded.
    pub degraded_components: Vec<&'static str>,
}

pub fn diagnostics_view(report: &RecoveryReport, health: &RuntimeHealth) -> DiagnosticsView {
    use RecoveryActionKind::*;
    let mut v = DiagnosticsView {
        recovered: Vec::new(),
        resumed: Vec::new(),
        paused: Vec::new(),
        degraded_components: Vec::new(),
    };
    for a in &report.repaired_items {
        match a.kind {
            TransferResumed | DtnBundlesRestored | OutboxReconstructed => v.resumed.push(*a),
            TransferPaused => v.paused.push(*a),
            ProjectionRebuilt
            | OrphanTempDeleted
            | PartialChunkReverified
            | ReservationReleased
            | BlobQuarantined => v.recovered.push(*a),
        }
    }
    for (name, s) in [
        ("storage", health.storage),
        ("identity", health.identity),
        ("messaging", health.messaging),
        ("files", health.files),
        ("dtn", health.dtn),
    ] {
        if s != HealthState::Healthy {
            v.degraded_components.push(name);
        }
    }
    v
}

// ---------------------------------------------------------------------
// §177
// ---------------------------------------------------------------------

/// §177: the FFI host app can crash independently. It reconnects
/// exactly like any IPC client — fresh snapshot first — because the
/// durable core, not the host, is the source of truth.
pub type FfiHostSession = IpcClient;

/// A host crash never changes durable state.
pub const HOST_CRASH_CHANGES_DURABLE_STATE: bool = false;
const _: () = assert!(!HOST_CRASH_CHANGES_DURABLE_STATE);

// ---------------------------------------------------------------------
// §178
// ---------------------------------------------------------------------

/// What a WASM component's storage can promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceSemantics {
    DurableAtomic,
    /// Writes may be lost or reordered on crash.
    BestEffort,
    VolatileOnly,
}

/// §178: the shared model still applies; what changes is how much of
/// it the environment can actually deliver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmRecoveryContract {
    /// Accepted intent survives a crash.
    pub intent_survives_crash: bool,
    /// Effects must carry stable ids and be idempotent — always.
    pub effects_idempotent: bool,
    /// Derived state must be rebuildable — always.
    pub projections_rebuildable: bool,
    /// The user/integrator must be told pending work can be lost.
    pub must_warn_pending_may_be_lost: bool,
}

pub const fn wasm_contract(p: PersistenceSemantics) -> WasmRecoveryContract {
    let durable = matches!(p, PersistenceSemantics::DurableAtomic);
    WasmRecoveryContract {
        intent_survives_crash: durable,
        effects_idempotent: true,
        projections_rebuildable: true,
        must_warn_pending_may_be_lost: !durable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery_mode::RecoverySeverity;

    fn hello(major: u16, minor: u16, feats: &[RecoveryFeature]) -> ProtocolHello {
        let mut f = FeatureSet::empty();
        for x in feats {
            f = f.with(*x);
        }
        ProtocolHello {
            major,
            minor,
            features: f,
        }
    }

    #[test]
    fn negotiation_takes_min_minor_and_the_feature_intersection() {
        use RecoveryFeature::*;
        let n = negotiate(
            hello(1, 5, &[Report, Health, Snapshot]),
            hello(1, 3, &[Report, Snapshot, Diagnostics]),
        )
        .unwrap();
        assert_eq!((n.major, n.minor), (1, 3));
        assert!(n.supports(Report) && n.supports(Snapshot));
        assert!(!n.supports(Health), "daemon lacks it");
        assert!(!n.supports(Diagnostics), "client lacks it");
        assert!(!n.supports(Admission));
    }

    #[test]
    fn a_gui_cannot_assume_recovered_state_an_older_daemon_does_not_have() {
        use RecoveryFeature::*;
        let old_daemon = hello(1, 0, &[Snapshot]);
        let new_gui = hello(1, 9, &[Report, Health, Snapshot, Admission, Diagnostics]);
        let n = negotiate(new_gui, old_daemon).unwrap();
        for f in [Report, Health, Admission, Diagnostics] {
            assert!(!n.supports(f), "{f:?} must not be assumed");
        }
        assert!(n.supports(Snapshot));
    }

    #[test]
    fn different_major_versions_do_not_negotiate() {
        assert_eq!(
            negotiate(hello(2, 0, &[]), hello(1, 0, &[])),
            Err(MajorMismatch {
                client: 2,
                daemon: 1
            })
        );
    }

    #[test]
    fn feature_set_basics() {
        let s = FeatureSet::empty().with(RecoveryFeature::Health);
        assert!(s.contains(RecoveryFeature::Health) && !s.contains(RecoveryFeature::Report));
        assert_eq!(s.intersect(FeatureSet::empty()), FeatureSet::empty());
    }

    // Release N: writes 3, reads 2..=4 (it reads ahead).
    const OLD: FormatSupport = FormatSupport {
        write_version: 3,
        min_read_version: 2,
        max_read_version: 4,
    };
    // Release N+1: writes 4, reads 3..=4.
    const NEW: FormatSupport = FormatSupport {
        write_version: 4,
        min_read_version: 3,
        max_read_version: 4,
    };

    #[test]
    fn adjacent_releases_can_run_side_by_side() {
        assert!(mutually_compatible(OLD, NEW));
        assert!(rolling_upgrade_ok(&[OLD, NEW, NEW, OLD]));
    }

    #[test]
    fn a_release_that_cannot_read_ahead_blocks_rolling_upgrade() {
        // Adjacent version numbers, but this node only reads what it
        // writes: it cannot read what NEW writes, so mixed operation
        // breaks.
        let no_read_ahead = FormatSupport {
            write_version: 3,
            min_read_version: 2,
            max_read_version: 3,
        };
        assert!(!mutually_compatible(no_read_ahead, NEW));
        assert!(!rolling_upgrade_ok(&[no_read_ahead, NEW]));
    }

    #[test]
    fn a_release_that_drops_old_formats_forces_a_simultaneous_upgrade() {
        let too_new = FormatSupport {
            write_version: 6,
            min_read_version: 5,
            max_read_version: 6,
        };
        assert!(!mutually_compatible(OLD, too_new));
        assert!(!rolling_upgrade_ok(&[OLD, NEW, too_new]));
    }

    #[test]
    fn a_node_reads_only_within_its_declared_window() {
        assert!(can_read(NEW, 4) && can_read(NEW, 3));
        assert!(!can_read(NEW, 5), "newer than it understands");
        assert!(!can_read(NEW, 2), "older than it still reads");
        assert!(rolling_upgrade_ok(&[]) && rolling_upgrade_ok(&[OLD]));
    }

    #[test]
    fn relays_never_persist_plaintext_or_user_keys() {
        assert!(relay_may_persist(RelayDurableKind::EncryptedQueue));
        assert!(relay_may_persist(RelayDurableKind::SessionMetadata));
        assert!(!relay_may_persist(RelayDurableKind::MessagePlaintext));
        assert!(!relay_may_persist(RelayDurableKind::UserE2eeKeys));
    }

    #[test]
    fn diagnostics_answer_recovered_resumed_and_degraded() {
        use RecoveryActionKind::*;
        let mut r = RecoveryReport::clean();
        r.severity = RecoverySeverity::Degraded;
        for (kind, count) in [
            (ProjectionRebuilt, 2),
            (TransferResumed, 3),
            (TransferPaused, 1),
            (DtnBundlesRestored, 12),
            (OrphanTempDeleted, 7),
        ] {
            r.repaired_items.push(RecoveryAction { kind, count });
        }
        let h = RuntimeHealth {
            recovery: RecoverySeverity::Degraded,
            storage: HealthState::Healthy,
            identity: HealthState::Healthy,
            messaging: HealthState::Healthy,
            files: HealthState::Degraded,
            dtn: HealthState::Unavailable,
        };
        let v = diagnostics_view(&r, &h);
        assert_eq!(v.recovered.len(), 2);
        assert_eq!(v.resumed.len(), 2);
        assert_eq!(v.paused.len(), 1);
        assert_eq!(v.degraded_components, vec!["files", "dtn"]);
    }

    #[test]
    fn every_action_kind_lands_in_exactly_one_bucket() {
        use RecoveryActionKind::*;
        let all = [
            ProjectionRebuilt,
            TransferResumed,
            TransferPaused,
            OrphanTempDeleted,
            PartialChunkReverified,
            DtnBundlesRestored,
            ReservationReleased,
            BlobQuarantined,
            OutboxReconstructed,
        ];
        let mut r = RecoveryReport::clean();
        for k in all {
            r.repaired_items.push(RecoveryAction { kind: k, count: 1 });
        }
        let h = RuntimeHealth {
            recovery: RecoverySeverity::Clean,
            storage: HealthState::Healthy,
            identity: HealthState::Healthy,
            messaging: HealthState::Healthy,
            files: HealthState::Healthy,
            dtn: HealthState::Healthy,
        };
        let v = diagnostics_view(&r, &h);
        assert_eq!(
            v.recovered.len() + v.resumed.len() + v.paused.len(),
            all.len()
        );
        assert!(v.degraded_components.is_empty());
    }

    #[test]
    fn a_crashed_ffi_host_reattaches_by_snapshot_not_by_replay() {
        use crate::shutdown_marker::RuntimeGeneration;
        let mut host: FfiHostSession = IpcClient::new();
        host.begin_reconnect().unwrap();
        host.connected().unwrap();
        assert!(host.resubscribed().is_err(), "cannot skip the snapshot");
        host.snapshot_received(RuntimeGeneration(4)).unwrap();
        host.resubscribed().unwrap();
        assert!(host.accept_event(RuntimeGeneration(4)).is_ok());
    }

    #[test]
    fn wasm_environments_keep_the_model_but_may_not_keep_the_intent() {
        let d = wasm_contract(PersistenceSemantics::DurableAtomic);
        assert!(d.intent_survives_crash && !d.must_warn_pending_may_be_lost);
        for p in [
            PersistenceSemantics::BestEffort,
            PersistenceSemantics::VolatileOnly,
        ] {
            let c = wasm_contract(p);
            assert!(!c.intent_survives_crash, "{p:?}");
            assert!(c.must_warn_pending_may_be_lost, "{p:?}");
        }
        for p in [
            PersistenceSemantics::DurableAtomic,
            PersistenceSemantics::BestEffort,
            PersistenceSemantics::VolatileOnly,
        ] {
            let c = wasm_contract(p);
            assert!(c.effects_idempotent && c.projections_rebuildable, "{p:?}");
        }
    }
}
