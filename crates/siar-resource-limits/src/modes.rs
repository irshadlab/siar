//! §146-152 runtime modes: server overload, headless relay, mobile
//! background, foreground, battery saver, emergency, call active.
//!
//! Modes are *flags that compose* (a phone can be backgrounded, in
//! battery saver, with a call active, and in an emergency all at once).
//! [`apply_modes`] turns the base tunables into the effective ones in a
//! fixed order: boosts first (foreground, emergency), then every
//! restriction, then a clamp to the hard ceilings. Order matters — a
//! restriction must always beat a boost — and the invariants below hold
//! for every one of the 2^6 flag combinations (tested exhaustively):
//! nothing exceeds a hard ceiling, critical reserves never shrink,
//! authenticated-control capacity is never reduced, and essential work
//! (≥1 transfer, ≥1 CPU permit) is never throttled to zero.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeTunables {
    pub transfer_concurrency: u64,
    pub discovery_concurrency: u64,
    pub scan_concurrency: u64,
    pub cpu_permits: u64,
    pub ui_cache_bytes: u64,
    pub background_cache_bytes: u64,
    /// 100 = full-size queues.
    pub queue_scale_percent: u64,
    pub bulk_rate_bytes_per_sec: u64,
    pub critical_queue_reserve: u64,
    pub dtn_critical_reserve: u64,
    pub unknown_peer_slots: u64,
    /// Capacity kept for authenticated control traffic. No mode lowers it.
    pub control_reserve: u64,
    /// What a call reserves *while one is active*; 0 otherwise.
    pub call_reserved_bandwidth: u64,
    pub call_reserved_cpu_permits: u64,
    pub call_reserved_memory: u64,
    pub accept_new_bulk: bool,
    pub background_sync_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModeFlags {
    pub foreground: bool,
    pub background: bool,
    pub battery_saver: bool,
    pub emergency: bool,
    pub call_active: bool,
    pub server_overload: bool,
    /// Deployment shape rather than a transient mode (§147): no UI exists.
    pub headless_relay: bool,
}

fn pct(v: u64, p: u64) -> u64 {
    (u128::from(v) * u128::from(p) / 100) as u64
}

/// `max(1, v * p / 100)` for things that must keep making progress.
fn pct_min1(v: u64, p: u64) -> u64 {
    if v == 0 {
        0
    } else {
        pct(v, p).max(1)
    }
}

pub fn apply_modes(base: &ModeTunables, hard: &ModeTunables, f: &ModeFlags) -> ModeTunables {
    let mut t = *base;
    // Calls reserve resources only while active.
    if !f.call_active {
        t.call_reserved_bandwidth = 0;
        t.call_reserved_cpu_permits = 0;
        t.call_reserved_memory = 0;
    }

    // ---- boosts (clamped to hard at the end) ----
    // §149: foreground may raise discovery, transfers, UI cache. If the
    // app is also flagged background (stale flag), background wins.
    if f.foreground && !f.background {
        t.discovery_concurrency = t.discovery_concurrency.saturating_mul(2);
        t.transfer_concurrency = t.transfer_concurrency.saturating_mul(2);
        t.ui_cache_bytes = t.ui_cache_bytes.saturating_mul(2);
    }
    // §151: emergency reallocates toward critical, away from caches.
    if f.emergency {
        t.critical_queue_reserve = t.critical_queue_reserve.saturating_mul(2);
        t.dtn_critical_reserve = t.dtn_critical_reserve.saturating_mul(2);
        t.background_cache_bytes /= 2;
    }

    // ---- restrictions (always applied after boosts) ----
    if f.background {
        // §148: reduce queues, pause bulk, reduce CPU jobs; keep
        // essential durable/control (reserves and control untouched).
        t.queue_scale_percent = pct(t.queue_scale_percent, 50);
        t.accept_new_bulk = false;
        t.cpu_permits = pct_min1(t.cpu_permits, 50);
        t.transfer_concurrency = pct_min1(t.transfer_concurrency, 50);
        t.ui_cache_bytes = 0;
    }
    if f.battery_saver {
        // §150: CPU permits, network bulk rates, scan concurrency.
        t.cpu_permits = pct_min1(t.cpu_permits, 50);
        t.bulk_rate_bytes_per_sec = pct(t.bulk_rate_bytes_per_sec, 25);
        t.scan_concurrency = pct_min1(t.scan_concurrency, 50);
    }
    if f.call_active {
        // §152: bandwidth/CPU/memory/AV buffers are reserved (above);
        // throttle bulk transfer and background sync.
        t.bulk_rate_bytes_per_sec = pct(t.bulk_rate_bytes_per_sec, 25);
        t.background_sync_allowed = false;
    }
    if f.server_overload {
        // §146: reject new bulk, limit unknown peers, preserve
        // authenticated control — degrade rather than crash.
        t.accept_new_bulk = false;
        t.unknown_peer_slots = pct(t.unknown_peer_slots, 25);
    }
    if f.headless_relay {
        // §147: no UI memory reserved.
        t.ui_cache_bytes = 0;
    }

    // ---- clamp to hard ceilings; protect reserves ----
    macro_rules! clamp { ($($field:ident),*) => { $( t.$field = t.$field.min(hard.$field); )* }; }
    clamp!(
        transfer_concurrency,
        discovery_concurrency,
        scan_concurrency,
        cpu_permits,
        ui_cache_bytes,
        background_cache_bytes,
        queue_scale_percent,
        bulk_rate_bytes_per_sec,
        critical_queue_reserve,
        dtn_critical_reserve,
        unknown_peer_slots,
        call_reserved_bandwidth,
        call_reserved_cpu_permits,
        call_reserved_memory
    );
    // A reserve is never reduced below the configured base by clamping
    // *or* by any mode; the hard ceiling can't be below base (validated
    // by the caller), so `max(base)` only guards against bad input.
    t.critical_queue_reserve = t
        .critical_queue_reserve
        .max(base.critical_queue_reserve.min(hard.critical_queue_reserve));
    t.dtn_critical_reserve = t
        .dtn_critical_reserve
        .max(base.dtn_critical_reserve.min(hard.dtn_critical_reserve));
    t.control_reserve = base.control_reserve;
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ModeTunables {
        ModeTunables {
            transfer_concurrency: 4,
            discovery_concurrency: 2,
            scan_concurrency: 4,
            cpu_permits: 4,
            ui_cache_bytes: 64,
            background_cache_bytes: 100,
            queue_scale_percent: 100,
            bulk_rate_bytes_per_sec: 1_000_000,
            critical_queue_reserve: 10,
            dtn_critical_reserve: 20,
            unknown_peer_slots: 16,
            control_reserve: 8,
            call_reserved_bandwidth: 500_000,
            call_reserved_cpu_permits: 1,
            call_reserved_memory: 32,
            accept_new_bulk: true,
            background_sync_allowed: true,
        }
    }

    fn hard() -> ModeTunables {
        ModeTunables {
            transfer_concurrency: 8,
            discovery_concurrency: 4,
            scan_concurrency: 8,
            cpu_permits: 8,
            ui_cache_bytes: 128,
            background_cache_bytes: 200,
            queue_scale_percent: 100,
            bulk_rate_bytes_per_sec: 2_000_000,
            critical_queue_reserve: 20,
            dtn_critical_reserve: 40,
            unknown_peer_slots: 32,
            control_reserve: 8,
            call_reserved_bandwidth: 1_000_000,
            call_reserved_cpu_permits: 2,
            call_reserved_memory: 64,
            accept_new_bulk: true,
            background_sync_allowed: true,
        }
    }

    fn flags(f: impl FnOnce(&mut ModeFlags)) -> ModeFlags {
        let mut m = ModeFlags::default();
        f(&mut m);
        m
    }

    fn all_combos() -> Vec<ModeFlags> {
        (0..128u32)
            .map(|b| ModeFlags {
                foreground: b & 1 != 0,
                background: b & 2 != 0,
                battery_saver: b & 4 != 0,
                emergency: b & 8 != 0,
                call_active: b & 16 != 0,
                server_overload: b & 32 != 0,
                headless_relay: b & 64 != 0,
            })
            .collect()
    }

    #[test]
    fn no_flags_means_base_except_call_reservations_which_only_exist_during_calls() {
        let t = apply_modes(&base(), &hard(), &ModeFlags::default());
        assert_eq!(
            t,
            ModeTunables {
                call_reserved_bandwidth: 0,
                call_reserved_cpu_permits: 0,
                call_reserved_memory: 0,
                ..base()
            }
        );
    }

    #[test]
    fn foreground_boosts_within_hard_limits() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.foreground = true));
        assert_eq!(
            (
                t.discovery_concurrency,
                t.transfer_concurrency,
                t.ui_cache_bytes
            ),
            (4, 8, 128)
        );
        // hard clamp: boosting past the ceiling is cut off
        let mut b = base();
        b.transfer_concurrency = 7;
        assert_eq!(
            apply_modes(&b, &hard(), &flags(|f| f.foreground = true)).transfer_concurrency,
            8
        );
    }

    #[test]
    fn background_pauses_bulk_and_shrinks_queues_but_keeps_essentials() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.background = true));
        assert!(!t.accept_new_bulk);
        assert_eq!(
            (t.queue_scale_percent, t.cpu_permits, t.ui_cache_bytes),
            (50, 2, 0)
        );
        assert_eq!(
            (
                t.critical_queue_reserve,
                t.dtn_critical_reserve,
                t.control_reserve
            ),
            (10, 20, 8)
        );
        assert!(t.transfer_concurrency >= 1);
    }

    #[test]
    fn background_beats_a_stale_foreground_flag() {
        let t = apply_modes(
            &base(),
            &hard(),
            &flags(|f| {
                f.foreground = true;
                f.background = true;
            }),
        );
        assert_eq!(
            t.discovery_concurrency, 2,
            "no foreground boost while backgrounded"
        );
        assert_eq!(t.ui_cache_bytes, 0);
    }

    #[test]
    fn battery_saver_tightens_cpu_bulk_rate_and_scans() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.battery_saver = true));
        assert_eq!(
            (t.cpu_permits, t.bulk_rate_bytes_per_sec, t.scan_concurrency),
            (2, 250_000, 2)
        );
    }

    #[test]
    fn emergency_shifts_toward_critical_reserves_and_away_from_caches() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.emergency = true));
        assert_eq!(
            (
                t.critical_queue_reserve,
                t.dtn_critical_reserve,
                t.background_cache_bytes
            ),
            (20, 40, 50)
        );
        // cannot exceed the global ceiling even if base is already high
        let mut b = base();
        b.critical_queue_reserve = 18;
        assert_eq!(
            apply_modes(&b, &hard(), &flags(|f| f.emergency = true)).critical_queue_reserve,
            20
        );
    }

    #[test]
    fn call_active_reserves_and_throttles_bulk_and_sync() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.call_active = true));
        assert_eq!(
            (
                t.call_reserved_bandwidth,
                t.call_reserved_cpu_permits,
                t.call_reserved_memory
            ),
            (500_000, 1, 32)
        );
        assert_eq!(t.bulk_rate_bytes_per_sec, 250_000);
        assert!(!t.background_sync_allowed);
    }

    #[test]
    fn server_overload_rejects_bulk_limits_unknowns_and_preserves_control() {
        let t = apply_modes(&base(), &hard(), &flags(|f| f.server_overload = true));
        assert!(!t.accept_new_bulk);
        assert_eq!(t.unknown_peer_slots, 4);
        assert_eq!(t.control_reserve, 8);
    }

    #[test]
    fn headless_relay_reserves_no_ui_memory() {
        let t = apply_modes(
            &base(),
            &hard(),
            &flags(|f| {
                f.headless_relay = true;
                f.foreground = true;
            }),
        );
        assert_eq!(t.ui_cache_bytes, 0);
    }

    #[test]
    fn invariants_hold_for_every_combination_of_modes() {
        let (b, h) = (base(), hard());
        for f in all_combos() {
            let t = apply_modes(&b, &h, &f);
            // never above a hard ceiling
            assert!(t.transfer_concurrency <= h.transfer_concurrency, "{f:?}");
            assert!(t.discovery_concurrency <= h.discovery_concurrency, "{f:?}");
            assert!(
                t.cpu_permits <= h.cpu_permits && t.ui_cache_bytes <= h.ui_cache_bytes,
                "{f:?}"
            );
            assert!(
                t.critical_queue_reserve <= h.critical_queue_reserve,
                "{f:?}"
            );
            assert!(t.dtn_critical_reserve <= h.dtn_critical_reserve, "{f:?}");
            assert!(
                t.call_reserved_bandwidth <= h.call_reserved_bandwidth,
                "{f:?}"
            );
            // critical reserves and control capacity never shrink
            assert!(
                t.critical_queue_reserve >= b.critical_queue_reserve,
                "{f:?}"
            );
            assert!(t.dtn_critical_reserve >= b.dtn_critical_reserve, "{f:?}");
            assert_eq!(t.control_reserve, b.control_reserve, "{f:?}");
            // essential work always possible
            assert!(
                t.cpu_permits >= 1 && t.transfer_concurrency >= 1 && t.scan_concurrency >= 1,
                "{f:?}"
            );
            // restrictions beat boosts: any restricting flag means no bulk growth
            if f.background || f.server_overload {
                assert!(!t.accept_new_bulk, "{f:?}");
            }
            if f.background {
                assert!(t.discovery_concurrency <= b.discovery_concurrency, "{f:?}");
            }
            // call reservations exist only during calls
            if !f.call_active {
                assert_eq!(t.call_reserved_bandwidth, 0, "{f:?}");
            }
        }
    }

    #[test]
    fn modes_are_idempotent_on_flags_not_cumulative_on_reapply() {
        // Re-deriving from base each time means toggling can't ratchet limits down.
        let (b, h) = (base(), hard());
        let f = flags(|f| {
            f.battery_saver = true;
            f.background = true;
        });
        let once = apply_modes(&b, &h, &f);
        let again = apply_modes(&b, &h, &f);
        assert_eq!(once, again);
        assert_eq!(
            apply_modes(&b, &h, &ModeFlags::default()).cpu_permits,
            b.cpu_permits,
            "restored when flags clear"
        );
    }
}
