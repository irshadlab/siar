//! §174 concurrency tests: many tasks acquiring and releasing resources.
//! Asserts: no deadlock (everything finishes within a deadline), no
//! starvation beyond policy (every worker completes its quota), no
//! accounting drift (every counter returns to exactly zero).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use siar_protocol_ext::TrafficPriority;
use siar_resource_limits::budget::{GlobalBudget, Subsystem};
use siar_resource_limits::connections::ConnectionLimiter;
use siar_resource_limits::fair_semaphore::FairSemaphore;
use siar_resource_limits::reservation::ResourcePools;
use siar_resource_limits::tasks::TaskRegistry;
use siar_resource_limits::types::ResourceKind;

/// Runs `f` on its own thread; fails the test if it doesn't finish
/// (i.e. a deadlock or livelock) within `secs`.
fn finishes_within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .expect("deadlock or livelock: did not finish in time")
}

#[test]
fn opposite_order_multi_resource_requests_cannot_deadlock() {
    // Classic deadlock shape: half the workers list (A, B), half (B, A),
    // and each resource alone is too small for two holders. Acquisition is
    // canonical-order and all-or-nothing, so there is no hold-and-wait.
    let pools = ResourcePools::new(&[(ResourceKind::Memory, 10), (ResourceKind::Streams, 10)]);
    let done = Arc::new(AtomicU64::new(0));
    let total = finishes_within(60, {
        let (pools, done) = (pools.clone(), Arc::clone(&done));
        move || {
            let hs: Vec<_> = (0..8)
                .map(|t| {
                    let (pools, done) = (pools.clone(), Arc::clone(&done));
                    thread::spawn(move || {
                        let req: [(ResourceKind, u64); 2] = if t % 2 == 0 {
                            [(ResourceKind::Memory, 8), (ResourceKind::Streams, 8)]
                        } else {
                            [(ResourceKind::Streams, 8), (ResourceKind::Memory, 8)]
                        };
                        let mut got = 0;
                        while got < 500 {
                            if let Ok(b) = pools.acquire(&req, None) {
                                // both held together or neither: never a partial hold
                                assert_eq!(b.amount(ResourceKind::Memory), 8);
                                got += 1;
                                done.fetch_add(1, Ordering::Relaxed);
                                drop(b);
                            } else {
                                thread::yield_now();
                            }
                        }
                    })
                })
                .collect();
            hs.into_iter().for_each(|h| h.join().unwrap());
            done.load(Ordering::Relaxed)
        }
    });
    assert_eq!(
        total,
        8 * 500,
        "no starvation: every worker completed its quota"
    );
    assert_eq!(
        (
            pools.in_use(ResourceKind::Memory),
            pools.in_use(ResourceKind::Streams)
        ),
        (0, 0),
        "no accounting drift"
    );
}

#[test]
fn mixed_primitives_under_load_end_with_zero_drift() {
    let pools = ResourcePools::new(&[(ResourceKind::Memory, 1_000), (ResourceKind::Cpu, 4)]);
    let budget = GlobalBudget::new(10_000, [2_000; 5], 5_000);
    let conns = ConnectionLimiter::new(16, 2);
    let sem = FairSemaphore::new(3);
    let registry = TaskRegistry::new(64);
    let completed = Arc::new(AtomicU64::new(0));

    finishes_within(90, {
        let (pools, budget, conns, sem, registry, completed) = (
            pools.clone(),
            budget.clone(),
            conns.clone(),
            sem.clone(),
            registry.clone(),
            Arc::clone(&completed),
        );
        move || {
            let hs: Vec<_> = (0..12u32)
                .map(|t| {
                    let (pools, budget, conns, sem, registry, completed) = (
                        pools.clone(),
                        budget.clone(),
                        conns.clone(),
                        sem.clone(),
                        registry.clone(),
                        Arc::clone(&completed),
                    );
                    thread::spawn(move || {
                        let mut peer = [0u8; 32];
                        peer[0] = (t % 6) as u8;
                        for i in 0..300u64 {
                            let _task =
                                registry.register("worker", TrafficPriority::Normal, i).ok();
                            let _c = conns.try_acquire(peer).ok();
                            let _b = budget
                                .try_acquire(Subsystem::ALL[(t % 5) as usize], 100 + i % 50)
                                .ok();
                            let _p = pools
                                .acquire(
                                    &[(ResourceKind::Memory, 50), (ResourceKind::Cpu, 1)],
                                    None,
                                )
                                .ok();
                            let mut w = sem.wait(1).unwrap();
                            let mut spins = 0u32;
                            let _s = loop {
                                if let Some(p) = w.poll() {
                                    break Some(p);
                                }
                                spins += 1;
                                if spins > 2_000_000 {
                                    break None;
                                }
                                thread::yield_now();
                            };
                            completed.fetch_add(1, Ordering::Relaxed);
                        }
                    })
                })
                .collect();
            hs.into_iter().for_each(|h| h.join().unwrap());
        }
    });

    assert_eq!(
        completed.load(Ordering::Relaxed),
        12 * 300,
        "every worker finished its quota"
    );
    assert_eq!(pools.in_use(ResourceKind::Memory), 0);
    assert_eq!(pools.in_use(ResourceKind::Cpu), 0);
    assert_eq!(budget.total_used(), 0);
    assert_eq!(conns.active(), 0);
    assert_eq!((sem.available(), sem.queued()), (3, 0));
    assert_eq!(registry.active(), 0);
}

#[test]
fn heavy_hitters_cannot_starve_a_light_waiter_on_a_fair_semaphore() {
    let sem = FairSemaphore::new(1);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heavy: Vec<_> = (0..3)
        .map(|_| {
            let (sem, stop) = (sem.clone(), Arc::clone(&stop));
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let mut w = sem.wait(1).unwrap();
                    loop {
                        if let Some(p) = w.poll() {
                            drop(p);
                            break;
                        }
                        thread::yield_now();
                    }
                }
            })
        })
        .collect();
    let light_done = finishes_within(60, {
        let sem = sem.clone();
        move || {
            let mut n = 0;
            for _ in 0..100 {
                let mut w = sem.wait(1).unwrap();
                loop {
                    if let Some(p) = w.poll() {
                        drop(p);
                        n += 1;
                        break;
                    }
                    thread::yield_now();
                }
            }
            n
        }
    });
    stop.store(true, Ordering::Relaxed);
    heavy.into_iter().for_each(|h| h.join().unwrap());
    assert_eq!(light_done, 100);
    assert_eq!(sem.available(), 1);
}
