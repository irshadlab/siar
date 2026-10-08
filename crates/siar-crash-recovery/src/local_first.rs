//! §132 "Recovery Should Not Depend on Network" and §133
//! "Peer-Assisted Repair".
//!
//! Minimum local recovery must finish with no Internet (offline and
//! emergency use is a design goal). Peer help is an *optional
//! afterward step*: it can fill gaps, but whether the app opens never
//! depends on it. [`open_state`] is deliberately a function whose
//! result is independent of `network_available` once local recovery is
//! done, and a test pins that.

use crate::cleanup_quarantine::CleanupPhase;
use crate::progressive::RecoveryTier;
use crate::recovery_mode::RecoverySeverity;

/// §132: no recovery tier needs the network.
pub const fn tier_requires_network(_t: RecoveryTier) -> bool {
    false
}

/// §132: no cleanup phase needs the network.
pub const fn cleanup_phase_requires_network(_p: CleanupPhase) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenState {
    /// Local critical recovery is still running.
    WaitingForLocalRecovery,
    /// Usable now, with whatever local data exists.
    OpenOffline,
    /// Usable now; peer-assisted repair/sync may proceed in the
    /// background.
    OpenAndSyncing,
    /// Recovery ended in a state where the app cannot open (§49).
    CannotOpen,
}

pub const fn open_state(
    local_critical_done: bool,
    severity: RecoverySeverity,
    network_available: bool,
) -> OpenState {
    if matches!(severity, RecoverySeverity::Fatal) {
        return OpenState::CannotOpen;
    }
    if !local_critical_done {
        return OpenState::WaitingForLocalRecovery;
    }
    if network_available {
        OpenState::OpenAndSyncing
    } else {
        OpenState::OpenOffline
    }
}

/// Whether the app is usable — independent of the network by
/// construction.
pub const fn is_usable(state: OpenState) -> bool {
    matches!(state, OpenState::OpenOffline | OpenState::OpenAndSyncing)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerRepairKind {
    MissingMessages,
    MissingBlobs,
    DeviceDirectory,
}

impl PeerRepairKind {
    /// Anything a peer supplies is verified against local authority
    /// (event signatures, blob hashes, directory certificates) before
    /// it is used — peers help, they don't dictate.
    pub const fn requires_local_verification(self) -> bool {
        true
    }
}

/// Gaps to ask peers about. Empty unless local recovery is complete
/// *and* a network exists: peers are never consulted to get started.
pub fn peer_repair_plan(
    local_recovery_complete: bool,
    network_available: bool,
    gaps: &[PeerRepairKind],
) -> Vec<PeerRepairKind> {
    if !(local_recovery_complete && network_available) {
        return Vec::new();
    }
    let mut v: Vec<PeerRepairKind> = Vec::with_capacity(gaps.len());
    for g in gaps {
        if !v.contains(g) {
            v.push(*g);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_in_local_recovery_needs_the_network() {
        for t in RecoveryTier::ALL {
            assert!(!tier_requires_network(t));
        }
        for p in CleanupPhase::ORDER {
            assert!(!cleanup_phase_requires_network(p));
        }
    }

    #[test]
    fn opening_never_depends_on_the_network() {
        use RecoverySeverity::*;
        for sev in [Clean, Repaired, Degraded, ReadOnly, Fatal] {
            for done in [false, true] {
                let off = open_state(done, sev, false);
                let on = open_state(done, sev, true);
                assert_eq!(is_usable(off), is_usable(on), "{sev:?} done={done}");
            }
        }
    }

    #[test]
    fn offline_recovery_reaches_a_usable_app() {
        let s = open_state(true, RecoverySeverity::Repaired, false);
        assert_eq!(s, OpenState::OpenOffline);
        assert!(is_usable(s));
    }

    #[test]
    fn sync_starts_only_after_local_recovery() {
        assert_eq!(
            open_state(false, RecoverySeverity::Clean, true),
            OpenState::WaitingForLocalRecovery
        );
        assert_eq!(
            open_state(true, RecoverySeverity::Clean, true),
            OpenState::OpenAndSyncing
        );
    }

    #[test]
    fn fatal_cannot_open_regardless_of_network() {
        for net in [false, true] {
            assert_eq!(
                open_state(true, RecoverySeverity::Fatal, net),
                OpenState::CannotOpen
            );
        }
    }

    #[test]
    fn peer_repair_needs_local_recovery_and_a_network() {
        use PeerRepairKind::*;
        let gaps = [MissingMessages, MissingBlobs, MissingMessages];
        assert!(peer_repair_plan(false, true, &gaps).is_empty());
        assert!(peer_repair_plan(true, false, &gaps).is_empty());
        assert_eq!(
            peer_repair_plan(true, true, &gaps),
            vec![MissingMessages, MissingBlobs]
        );
        assert!(MissingMessages.requires_local_verification());
        assert!(DeviceDirectory.requires_local_verification());
    }
}
