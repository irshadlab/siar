//! Part 13 inside the application core: the one place where the core asks
//! "may I do this work now?", and where deferred work waits and resumes.
//!
//! What it gates (and what it deliberately does not):
//!
//! * **Attachment previews** — a network fetch plus an image decode. Always
//!   treated as *user-initiated* (the user is looking at the message), so low
//!   battery or saver never blocks them; only hard safety does (thermal
//!   critical, critical-battery reserve for large blobs). Under
//!   `RunThrottled` previews are serialized to one at a time.
//! * **History preload** — speculative background work at start-up. Deferred
//!   freely; nothing is lost because opening a conversation loads its history
//!   on demand.
//! * **Messages are never gated.** Sending, receiving, ACKs and retries bypass
//!   this module entirely: "Messages are never delayed to save power" is what
//!   the Settings card promises, so the code must make that true.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use siar_domain::{AttachmentReference, MessageId};
use siar_messaging::PeerTicket;
use siar_power::{
    energy_for_bytes, AppActivityState, Deployment, EnergyClass, OpId, PowerAwareWork,
    PowerDecision, PowerPolicyConfig, PowerScheduler, RawPowerReading, SchedulerConfig, WorkDomain,
    WorkPriority,
};
use siar_ui_model::PowerStatus;
use tokio::sync::Semaphore;

/// Work the gate can hold and hand back.
#[derive(Debug, Clone)]
pub enum Parked {
    Preview(Box<PreviewJob>),
    Preload,
}

/// Everything needed to (re)start one attachment-preview fetch.
#[derive(Debug, Clone)]
pub struct PreviewJob {
    pub peer: PeerTicket,
    pub message_id: MessageId,
    pub reference: AttachmentReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Run,
    /// Run, but one-at-a-time (see [`PowerGate::throttle`]).
    RunThrottled,
    /// Held; it comes back through [`PowerGate::release_due`].
    Parked,
}

pub struct PowerGate {
    sched: Mutex<PowerScheduler>,
    parked: Mutex<HashMap<OpId, Parked>>,
    next_id: AtomicU64,
    start: Instant,
    throttle: Arc<Semaphore>,
}

impl PowerGate {
    #[must_use]
    pub fn new(deployment: Deployment) -> Arc<Self> {
        Self::with_scheduler(deployment, SchedulerConfig::default())
    }

    /// Custom scheduler settings (tests use `herd_window_ms: 0`).
    #[must_use]
    pub fn with_scheduler(deployment: Deployment, cfg: SchedulerConfig) -> Arc<Self> {
        Arc::new(Self {
            sched: Mutex::new(PowerScheduler::new(
                PowerPolicyConfig {
                    deployment,
                    ..Default::default()
                },
                cfg,
            )),
            parked: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            start: Instant::now(),
            throttle: Arc::new(Semaphore::new(1)),
        })
    }

    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn sched(&self) -> std::sync::MutexGuard<'_, PowerScheduler> {
        self.sched.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn parked(&self) -> std::sync::MutexGuard<'_, HashMap<OpId, Parked>> {
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Feed one platform reading (tests call this directly).
    pub fn observe(&self, raw: &RawPowerReading) {
        let now = self.now();
        self.sched().observe(raw, now);
    }

    /// Read the OS power state (Linux sysfs; elsewhere a mains-powered
    /// default) and feed it in. The desktop app is, by definition, in the
    /// foreground when this runs on its own timer.
    pub fn sample_system(&self) {
        let mut raw = siar_power::platform::read_linux_sysfs(
            std::path::Path::new("/sys"),
            siar_power::platform::ThermalThresholds::default(),
        );
        raw.activity = AppActivityState::ForegroundIdle;
        let now = self.now();
        let mut s = self.sched();
        s.observe(&raw, now);
        s.tick(now);
    }

    /// Plain-language status for the Settings card.
    #[must_use]
    pub fn status(&self) -> PowerStatus {
        siar_ui_model::describe_power(&self.sched().diagnostics())
    }

    /// Ask permission for `work`; `Parked` means the gate now holds `what`.
    pub fn admit(&self, work: PowerAwareWork, what: Parked) -> Admission {
        let id = OpId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let now = self.now();
        let mut s = self.sched();
        match s.submit(id, work, now) {
            Ok(sub) => match sub.evaluation.decision {
                PowerDecision::Run => Admission::Run,
                PowerDecision::RunThrottled => Admission::RunThrottled,
                PowerDecision::Defer => {
                    self.parked().insert(id, what);
                    Admission::Parked
                }
                // Neither work type here is ephemeral; if one ever became so,
                // dropping it is the policy's explicit answer.
                PowerDecision::Drop | PowerDecision::Reject => Admission::Parked,
            },
            // Full or duplicate: fail open — never wedge the app on the gate.
            Err(_) => Admission::Run,
        }
    }

    /// Hand back whatever may run now, with the decision it earned.
    pub fn release_due(&self) -> Vec<(Parked, Admission)> {
        let now = self.now();
        let released = self.sched().poll(now, 8);
        let mut parked = self.parked();
        released
            .into_iter()
            .filter_map(|r| {
                let what = parked.remove(&r.id)?;
                let adm = if r.decision == PowerDecision::RunThrottled {
                    Admission::RunThrottled
                } else {
                    Admission::Run
                };
                Some((what, adm))
            })
            .collect()
    }

    /// One-at-a-time guard for `RunThrottled` work.
    #[must_use]
    pub fn throttle(&self) -> Arc<Semaphore> {
        self.throttle.clone()
    }

    #[must_use]
    pub fn parked_len(&self) -> usize {
        self.parked().len()
    }
}

/// Work descriptor for fetching and decoding one attachment preview.
#[must_use]
pub fn preview_work(reference: &AttachmentReference) -> PowerAwareWork {
    // Large blobs cost more battery and radio; a thumbnail is cheap.
    let energy = match energy_for_bytes(reference.encrypted_size.bytes()) {
        EnergyClass::Tiny | EnergyClass::Low => EnergyClass::Low,
        e => e,
    };
    PowerAwareWork::new(WorkDomain::Media, WorkPriority::Interactive, energy)
        .with_durable(true)
        .user_initiated()
}

/// Work descriptor for the start-up history preload (local reads only).
#[must_use]
pub fn preload_work() -> PowerAwareWork {
    PowerAwareWork::new(
        WorkDomain::Indexing,
        WorkPriority::Background,
        EnergyClass::Low,
    )
    .with_durable(true)
    .with_network(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_power::{ChargingState, ThermalState};

    fn reading(pct: u8, charging: ChargingState, thermal: ThermalState) -> RawPowerReading {
        RawPowerReading {
            battery_percent: Some(pct),
            charging,
            thermal,
            activity: AppActivityState::ForegroundIdle,
            ..Default::default()
        }
    }

    fn reference(bytes: u64) -> AttachmentReference {
        use siar_domain::{BlobSize, MediaType};
        AttachmentReference {
            blob_hash: [0; 32],
            encrypted_size: BlobSize::parse(bytes).unwrap(),
            media_type: MediaType::ImageJpeg,
            attachment_key: [0; 32],
            thumbnail: None,
        }
    }

    #[test]
    fn mains_powered_desktop_runs_everything_immediately() {
        let g = PowerGate::new(Deployment::Desktop);
        g.observe(&reading(100, ChargingState::Full, ThermalState::Nominal));
        assert_eq!(g.admit(preload_work(), Parked::Preload), Admission::Run);
        assert_eq!(
            g.admit(preview_work(&reference(2_000_000)), Parked::Preload),
            Admission::Run
        );
        assert_eq!(g.parked_len(), 0);
    }

    #[test]
    fn preload_waits_on_low_battery_but_previews_do_not() {
        let g = PowerGate::new(Deployment::Desktop);
        g.observe(&reading(
            15,
            ChargingState::NotCharging,
            ThermalState::Nominal,
        ));
        assert_eq!(g.admit(preload_work(), Parked::Preload), Admission::Parked);
        assert!(
            g.admit(preview_work(&reference(500_000)), Parked::Preload) != Admission::Parked,
            "the user is looking at that image — saver must not block it"
        );
    }

    #[test]
    fn preview_is_held_while_very_hot() {
        let g = PowerGate::new(Deployment::Desktop);
        g.observe(&reading(80, ChargingState::Full, ThermalState::Critical));
        assert_eq!(
            g.admit(preview_work(&reference(300_000)), Parked::Preload),
            Admission::Parked
        );
        assert!(
            g.release_due().is_empty(),
            "still hot: nothing is released, even though the user asked"
        );
        assert_eq!(g.parked_len(), 1, "held, not lost");
        // Thermal recovery itself (one step per dwell) is tested in siar-power.
    }

    #[test]
    fn critical_battery_holds_a_large_preview_but_not_a_small_one() {
        let g = PowerGate::new(Deployment::Desktop);
        g.observe(&reading(
            4,
            ChargingState::NotCharging,
            ThermalState::Nominal,
        ));
        // Tiny + user-initiated is inside the reserve: it runs (throttled, one at a time).
        assert!(g.admit(preview_work(&reference(900)), Parked::Preload) != Admission::Parked);
        assert_eq!(
            g.admit(preview_work(&reference(40_000_000)), Parked::Preload),
            Admission::Parked
        );
    }

    #[test]
    fn plugging_in_releases_parked_work() {
        let g = PowerGate::with_scheduler(
            Deployment::Desktop,
            SchedulerConfig {
                herd_window_ms: 0,
                ..Default::default()
            },
        );
        g.observe(&reading(
            4,
            ChargingState::NotCharging,
            ThermalState::Nominal,
        ));
        assert_eq!(g.admit(preload_work(), Parked::Preload), Admission::Parked);
        assert!(g.release_due().is_empty());
        g.observe(&reading(
            4,
            ChargingState::ChargingFast,
            ThermalState::Nominal,
        ));
        let out = g.release_due();
        assert_eq!(out.len(), 1, "work resumes once conditions allow");
        assert!(matches!(out[0].0, Parked::Preload));
        assert_eq!(g.parked_len(), 0);
    }

    #[test]
    fn status_text_never_contains_a_percentage() {
        let g = PowerGate::new(Deployment::Desktop);
        g.observe(&reading(
            37,
            ChargingState::NotCharging,
            ThermalState::Nominal,
        ));
        let s = g.status();
        let all = format!("{} {}", s.headline, s.details_text());
        assert!(!all.contains("37") && !all.contains('%'), "{all}");
    }
}
