//! §120-§129: scenario tests, one per scenario the spec names, driven through
//! the public API exactly as a host would.

use siar_power::*;

fn raw(pct: u8, charging: ChargingState, activity: AppActivityState) -> RawPowerReading {
    RawPowerReading {
        battery_percent: Some(pct),
        charging,
        thermal: ThermalState::Nominal,
        activity,
        ..Default::default()
    }
}
fn sched() -> PowerScheduler {
    PowerScheduler::new(
        PowerPolicyConfig::default(),
        SchedulerConfig {
            herd_window_ms: 0,
            ..Default::default()
        },
    )
}
fn files(e: EnergyClass) -> PowerAwareWork {
    PowerAwareWork::new(WorkDomain::Files, WorkPriority::Bulk, e)
}
fn advisor(s: &PowerScheduler) -> (PowerState, PowerBudget, PowerPolicyProfile) {
    let t = s.tracker();
    (*t.state(), *t.budget(), t.profile())
}

/// §122: battery High, start a big file; battery falls to Low → concurrency
/// drops; Saver → background chunking pauses; charger → resumes.
#[test]
fn five_gb_file_across_battery_changes() {
    let fg = AppActivityState::ForegroundIdle;
    let mut s = sched();
    s.observe(&raw(80, ChargingState::NotCharging, fg), 0);
    let (st, b, p) = advisor(&s);
    let high = PowerAdvisor::new(&st, &b, p).files().chunk_parallelism;
    assert!(s
        .submit(OpId(1), files(EnergyClass::High), 1)
        .unwrap()
        .evaluation
        .decision
        .runs());

    s.observe(&raw(18, ChargingState::NotCharging, fg), 2);
    let (st, b, p) = advisor(&s);
    let low = PowerAdvisor::new(&st, &b, p).files().chunk_parallelism;
    assert!(
        low < high,
        "Low battery reduces concurrency ({high} → {low})"
    );

    // New background chunks are now deferred (saver pauses background chunking).
    let sub = s.submit(OpId(2), files(EnergyClass::High), 3).unwrap();
    assert_eq!(sub.evaluation.decision, PowerDecision::Defer);

    s.observe(&raw(18, ChargingState::ChargingFast, fg), 4);
    let r = s.poll(4, 10);
    assert_eq!(r.len(), 1, "plugging in resumes the deferred chunking");
}

/// §123: Wi-Fi + cellular, battery Low ⇒ single path, unless emergency.
#[test]
fn multipath_is_single_path_on_low_battery_unless_emergency() {
    let mut s = sched();
    s.observe(
        &raw(
            15,
            ChargingState::NotCharging,
            AppActivityState::ForegroundInteractive,
        ),
        0,
    );
    assert!(!s.tracker().budget().multipath_allowed);
    s.set_emergency(true);
    assert!(
        s.tracker().budget().multipath_allowed,
        "emergency may use every path"
    );
}

/// §124: an AV1 software call with thermal Serious reduces resolution/fps or
/// goes audio-only — never silent.
#[test]
fn av1_call_under_serious_thermal_degrades_video_not_audio() {
    let mut s = sched();
    let hot = RawPowerReading {
        thermal: ThermalState::Serious,
        charging: ChargingState::Full,
        activity: AppActivityState::ForegroundInteractive,
        ..Default::default()
    };
    s.observe(&hot, 0);
    let (st, b, p) = advisor(&s);
    let m = PowerAdvisor::new(&st, &b, p).media();
    assert_eq!(m.av1_software, domains::Av1Software::Disabled);
    assert!(m.max_video <= domains::VideoCap::Low);
    assert!(m.audio_protected);
    assert!(ux::thermal_call_notice(st.thermal).is_some());
    let av1 = PowerAwareWork::new(
        WorkDomain::Media,
        WorkPriority::Interactive,
        EnergyClass::VeryHigh,
    )
    .with_durable(true);
    assert_eq!(
        s.submit(OpId(1), av1, 1).unwrap().evaluation.decision,
        PowerDecision::Defer,
        "software encode is held back"
    );
}

/// §125: battery Critical ⇒ critical bundles continue, bulk relay stops.
#[test]
fn dtn_on_critical_battery_keeps_critical_bundles_and_stops_bulk_relay() {
    let mut s = sched();
    s.observe(
        &raw(
            4,
            ChargingState::NotCharging,
            AppActivityState::BackgroundAllowed,
        ),
        0,
    );
    let sos = PowerAwareWork::new(WorkDomain::Dtn, WorkPriority::Critical, EnergyClass::Tiny);
    let bulk = PowerAwareWork::new(WorkDomain::Relay, WorkPriority::Bulk, EnergyClass::Medium);
    assert!(s
        .submit(OpId(1), sos, 1)
        .unwrap()
        .evaluation
        .decision
        .runs());
    assert_eq!(
        s.submit(OpId(2), bulk, 1).unwrap().evaluation.decision,
        PowerDecision::Defer
    );
    let (st, b, p) = advisor(&s);
    assert_eq!(
        PowerAdvisor::new(&st, &b, p).dtn().relay_intensity,
        domains::RelayIntensity::Off
    );
}

/// §126: Saver ⇒ BLE duty cycle reduced, Wi-Fi escalation rare.
#[test]
fn saver_reduces_ble_duty_and_wifi_escalation() {
    let mut s = sched();
    s.observe(
        &raw(
            70,
            ChargingState::ChargingFast,
            AppActivityState::ForegroundInteractive,
        ),
        0,
    );
    let (st, b, p) = advisor(&s);
    let normal = discovery::plan(p, &st, &b);
    let saver = PowerPolicyConfig {
        profile_hint: Some(PowerPolicyProfile::Saver),
        ..Default::default()
    };
    s.set_policy(saver);
    let (st2, b2, p2) = advisor(&s);
    assert_eq!(p2, PowerPolicyProfile::Saver);
    let reduced = discovery::plan(p2, &st2, &b2);
    assert!(reduced.duty_percent() < normal.duty_percent());
    assert_eq!(reduced.escalation, discovery::Escalation::BleOnly);
    assert!(!discovery::worth_upgrading(100 << 20, p2, &reduced));
}

/// §127: charging begins ⇒ deferred indexing/backups become eligible — but
/// resource limits are still enforced.
#[test]
fn charging_makes_deferred_indexing_eligible_but_resource_pressure_still_wins() {
    let mut s = sched();
    s.observe(
        &raw(
            15,
            ChargingState::NotCharging,
            AppActivityState::ForegroundIdle,
        ),
        0,
    );
    let idx = PowerAwareWork::new(
        WorkDomain::Indexing,
        WorkPriority::Background,
        EnergyClass::Medium,
    )
    .with_durable(true); // a deferred index job is one we want back (default: ephemeral)
    let backup = PowerAwareWork::new(
        WorkDomain::Backup,
        WorkPriority::Background,
        EnergyClass::High,
    );
    s.submit(OpId(1), idx, 1).unwrap();
    s.submit(OpId(2), backup, 1).unwrap();
    assert_eq!(s.deferred_len(), 2);

    s.set_pressure(ResourcePressure::High); // Part 08 says "no"
    s.observe(
        &raw(
            15,
            ChargingState::ChargingFast,
            AppActivityState::ForegroundIdle,
        ),
        2,
    );
    assert!(
        s.poll(2, 10).is_empty(),
        "charging cannot exceed hard resource limits"
    );

    s.set_pressure(ResourcePressure::Normal);
    assert_eq!(s.poll(3, 10).len(), 2, "both eligible once resources allow");
}

/// §128: background-restricted ⇒ non-critical work deferred, and **no busy
/// loop**: polling with unchanged inputs does no re-evaluation work.
#[test]
fn background_restricted_defers_without_busy_looping() {
    let mut s = sched();
    s.observe(
        &raw(
            80,
            ChargingState::NotCharging,
            AppActivityState::BackgroundRestricted,
        ),
        0,
    );
    for i in 0..200 {
        let w = PowerAwareWork::new(WorkDomain::Sync, WorkPriority::Bulk, EnergyClass::Medium);
        assert_eq!(
            s.submit(OpId(i), w, 1).unwrap().evaluation.decision,
            PowerDecision::Defer
        );
    }
    let before = s.diagnostics().metrics;
    for t in 2..2_000u64 {
        assert!(s.poll(t, 50).is_empty());
    }
    assert_eq!(
        s.diagnostics().metrics,
        before,
        "2000 polls with nothing changed touched no counters"
    );
    assert_eq!(s.deferred_len(), 200);
    let small = PowerAwareWork::new(WorkDomain::Message, WorkPriority::Normal, EnergyClass::Tiny);
    assert!(
        s.submit(OpId(9_999), small, 3)
            .unwrap()
            .evaluation
            .decision
            .runs(),
        "small messages still go"
    );
}

/// §129: kill while a transfer is deferred by power; restart; it is still
/// deferred or eligible based on the *new* state.
#[test]
fn process_kill_then_restart_reevaluates_against_the_new_state() {
    let mut before = sched();
    before.observe(
        &raw(
            10,
            ChargingState::NotCharging,
            AppActivityState::BackgroundAllowed,
        ),
        0,
    );
    before.submit(OpId(1), files(EnergyClass::High), 1).unwrap();
    let saved = before.persisted();
    drop(before); // the "kill"

    // Restart while still low: still deferred.
    let mut still_low = sched();
    still_low.observe(
        &raw(
            10,
            ChargingState::NotCharging,
            AppActivityState::BackgroundAllowed,
        ),
        100,
    );
    assert_eq!(still_low.restore(saved.clone()), 1);
    assert!(still_low.poll(101, 10).is_empty());
    assert_eq!(still_low.deferred_len(), 1);

    // Restart while charging: eligible.
    let mut charging = sched();
    charging.observe(
        &raw(
            10,
            ChargingState::ChargingFast,
            AppActivityState::BackgroundAllowed,
        ),
        100,
    );
    assert_eq!(charging.restore(saved), 1);
    assert_eq!(charging.poll(101, 10).len(), 1);
}

/// §121 (named invariant): critical durable work is never silently dropped
/// by power policy alone.
#[test]
fn critical_durable_work_is_never_dropped_by_power() {
    let states = [
        raw(
            2,
            ChargingState::NotCharging,
            AppActivityState::SuspendedLikely,
        ),
        RawPowerReading {
            thermal: ThermalState::Critical,
            ..raw(
                50,
                ChargingState::Full,
                AppActivityState::BackgroundRestricted,
            )
        },
    ];
    for st in states {
        let mut s = sched();
        s.observe(&st, 0);
        let w = PowerAwareWork::new(
            WorkDomain::Message,
            WorkPriority::Critical,
            EnergyClass::Tiny,
        )
        .with_durable(true);
        let d = s.submit(OpId(1), w, 1).unwrap().evaluation.decision;
        assert!(
            matches!(
                d,
                PowerDecision::Run | PowerDecision::RunThrottled | PowerDecision::Defer
            ),
            "{d:?}"
        );
    }
}

/// §133: the scheduler stays cheap with many deferrals.
#[test]
fn ten_thousand_deferrals_poll_quickly_when_nothing_changed() {
    let mut s = PowerScheduler::new(
        PowerPolicyConfig::default(),
        SchedulerConfig {
            max_deferred: 20_000,
            herd_window_ms: 0,
            ..Default::default()
        },
    );
    s.observe(
        &raw(
            10,
            ChargingState::NotCharging,
            AppActivityState::BackgroundAllowed,
        ),
        0,
    );
    for i in 0..10_000u64 {
        s.submit(OpId(i), files(EnergyClass::Medium), 1).unwrap();
    }
    s.poll(2, 10); // first poll evaluates once and caches
    let t0 = std::time::Instant::now();
    for t in 3..103u64 {
        assert!(s.poll(t, 10).is_empty());
    }
    let per_poll = t0.elapsed() / 100;
    assert!(
        per_poll < std::time::Duration::from_millis(50),
        "poll of 10k unchanged deferrals took {per_poll:?}"
    );
}
