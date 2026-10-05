//! §185 benchmarks, `harness = false`, std-only (no criterion in the
//! workspace). Run with `cargo bench -p siar-resource-limits`.
//!
//! Covers the spec's list: permit acquisition, queue operations,
//! scheduler throughput, 1000 peers, 10k deferred operations. The point
//! is the §185 requirement that the resource layer stay cheap relative
//! to network/storage work — these are sub-microsecond to microsecond
//! operations against I/O measured in milliseconds.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use siar_protocol_ext::TrafficPriority;
use siar_resource_limits::aging::{AgingPolicy, AgingQueue};
use siar_resource_limits::budget::{GlobalBudget, Subsystem};
use siar_resource_limits::connections::ConnectionLimiter;
use siar_resource_limits::event_log_pressure::{OutboxAccounting, OutboxClass, OutboxLimits};
use siar_resource_limits::metrics::ResourceMetrics;
use siar_resource_limits::peer_quota::TrustClass;
use siar_resource_limits::queue::QueueCapacities;
use siar_resource_limits::reservation::ResourcePools;
use siar_resource_limits::scheduler::{Job, Scheduler, ShardKind};
use siar_resource_limits::sharded::ShardedCounter;
use siar_resource_limits::token_bucket::TokenBucket;
use siar_resource_limits::types::ResourceKind;
use siar_resource_limits::verification::{VerificationBudget, VerifyLimits};

const SAMPLES: usize = 7;

/// Median / min / max of `SAMPLES` timed runs (ns per op). A single run
/// is dominated by scheduler noise; the median is what to read.
fn summarize(name: &str, mut ns_per_op: Vec<f64>, note: &str) {
    ns_per_op.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (min, med, max) = (
        ns_per_op[0],
        ns_per_op[ns_per_op.len() / 2],
        ns_per_op[ns_per_op.len() - 1],
    );
    println!("{name:<46} median {med:>8.1} ns/op  [min {min:>7.1} .. max {max:>7.1}]  {note}");
}

fn bench<F: FnMut()>(name: &str, iters: u64, mut f: F) {
    for _ in 0..(iters / 10).max(1) {
        f(); // warm-up
    }
    let runs = (0..SAMPLES)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..iters {
                f();
            }
            t.elapsed().as_nanos() as f64 / iters as f64
        })
        .collect();
    summarize(name, runs, &format!("({iters} iters x {SAMPLES})"));
}

/// For benchmarks that need fresh state per sample.
fn bench_fn(name: &str, ops: u64, mut run: impl FnMut() -> std::time::Duration) {
    let runs = (0..SAMPLES)
        .map(|_| run().as_nanos() as f64 / ops as f64)
        .collect();
    summarize(name, runs, &format!("({ops} ops x {SAMPLES})"));
}

fn main() {
    println!("siar-resource-limits benchmarks (std::time, single run; indicative only)\n");

    let pools = ResourcePools::new(&[
        (ResourceKind::Memory, u64::MAX / 2),
        (ResourceKind::Streams, 1 << 20),
    ]);
    bench("permit bundle acquire+release (2 kinds)", 1_000_000, || {
        black_box(
            pools
                .acquire(
                    black_box(&[(ResourceKind::Memory, 4096), (ResourceKind::Streams, 1)]),
                    None,
                )
                .ok(),
        );
    });

    let budget = GlobalBudget::new(1 << 40, [1 << 38; 5], 1 << 39);
    bench("global budget acquire+release", 1_000_000, || {
        black_box(budget.try_acquire(Subsystem::Files, black_box(4096)).ok());
    });

    let mut bucket = TokenBucket::new(1 << 30, 1 << 20, 0);
    let mut now = 0u64;
    bench("token bucket try_consume", 5_000_000, || {
        now += 1;
        black_box(bucket.try_consume(1, now));
    });

    let caps = QueueCapacities {
        critical: 1 << 20,
        control: 1 << 20,
        interactive: 1 << 20,
        normal: 1 << 20,
        bulk: 1 << 20,
        background: 1 << 20,
    };
    let mut q: AgingQueue<u64> = AgingQueue::new(caps, AgingPolicy::default());
    let prios = [
        TrafficPriority::Normal,
        TrafficPriority::Bulk,
        TrafficPriority::Critical,
        TrafficPriority::Interactive,
    ];
    let mut i = 0u64;
    bench("aging queue enqueue+dequeue", 2_000_000, || {
        i += 1;
        q.enqueue(prios[(i % 4) as usize], i, true, i);
        black_box(q.dequeue(i));
    });

    bench_fn("scheduler submit+dispatch (Vec demand)", 300_000, || {
        let metrics = Arc::new(ResourceMetrics::new());
        let sched: Scheduler<u64> = Scheduler::new(
            caps,
            AgingPolicy::default(),
            ResourcePools::new(&[(ResourceKind::Cpu, 1 << 30)]),
            metrics,
        );
        let t = Instant::now();
        for n in 0..300_000u64 {
            let job = Job {
                payload: n,
                priority: TrafficPriority::Normal,
                durable: true,
                demand: vec![(ResourceKind::Cpu, 1)],
            };
            sched.submit(ShardKind::Cpu, job, TrustClass::Known, n);
            black_box(sched.dispatch(ShardKind::Cpu, n));
        }
        t.elapsed()
    });

    // 1000 peers
    let limiter = ConnectionLimiter::new(100_000, 4);
    let keys: Vec<[u8; 32]> = (0..1_000u32)
        .map(|i| {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_le_bytes());
            k
        })
        .collect();
    let mut idx = 0usize;
    bench(
        "1000 peers: connection permit acquire+release",
        1_000_000,
        || {
            idx = (idx + 1) % keys.len();
            black_box(limiter.try_acquire(keys[idx]).ok());
        },
    );
    let vb = VerificationBudget::new(
        VerifyLimits {
            queue_slots: 1 << 20,
            unknown_slots: 1 << 19,
            unknown_class: (u64::MAX / 4, u64::MAX / 4),
            max_tracked_peers: 2_000,
        },
        0,
    );
    bench("1000 peers: verification submit (known)", 500_000, || {
        idx = (idx + 1) % keys.len();
        black_box(vb.submit(keys[idx], TrustClass::Known, 1, 0).ok());
    });

    // 10k deferred operations
    bench_fn("10k deferred ops: outbox add", 10_000, || {
        let mut outbox = OutboxAccounting::new(OutboxLimits {
            max_count: 20_000,
            max_bytes: u64::MAX / 2,
            critical_reserve_count: 100,
            critical_reserve_bytes: 0,
            bulk_percent: 100,
        });
        let t = Instant::now();
        for k in 0..10_000u64 {
            black_box(
                outbox
                    .try_add(OutboxClass::Normal, 100, 1_000_000 + k, 0)
                    .unwrap(),
            );
        }
        t.elapsed()
    });
    bench_fn(
        "10k deferred ops: expire-scan (whole table)",
        10_000,
        || {
            let mut outbox = OutboxAccounting::new(OutboxLimits {
                max_count: 20_000,
                max_bytes: u64::MAX / 2,
                critical_reserve_count: 100,
                critical_reserve_bytes: 0,
                bulk_percent: 100,
            });
            for k in 0..10_000u64 {
                outbox
                    .try_add(OutboxClass::Normal, 100, 1_000_000 + k, 0)
                    .unwrap();
            }
            let t = Instant::now();
            black_box(outbox.expire(1_000_000 + 5_000).len());
            t.elapsed()
        },
    );

    let sc = ShardedCounter::new(8);
    bench("sharded counter add (single thread)", 20_000_000, || {
        sc.add(black_box(1))
    });
    bench_fn(
        "sharded counter add (8 threads, per op wall)",
        16_000_000,
        || {
            let shared = Arc::new(ShardedCounter::new(8));
            let t = Instant::now();
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let s = Arc::clone(&shared);
                    std::thread::spawn(move || {
                        for _ in 0..2_000_000 {
                            s.add(1);
                        }
                    })
                })
                .collect();
            for h in workers {
                h.join().unwrap();
            }
            assert_eq!(shared.sum(), 16_000_000);
            t.elapsed()
        },
    );
}
