//! §156 scheduler architecture, §157 scheduler shards.
//!
//! ```text
//! Ingress → Admission → Priority queues → Fair scheduler
//!         → Resource permit acquisition → Worker → Feedback
//! ```
//! - **Admission** (shared, lock-free): trust-clamped priority
//!   ([`authorize_priority`]) and mode-based load shedding ([`shed`]).
//! - **Priority queues + fair scheduler**: one [`AgingQueue`] per shard,
//!   so waiting work climbs tiers instead of starving.
//! - **Permit acquisition**: a job is dispatched only when *all* its
//!   resources are acquired at once ([`ResourcePools::acquire`]); a job
//!   that can't be afforded stays where it is, keeping its age, and holds
//!   nothing while it waits.
//! - **Feedback**: queue wait and shed counts go to [`ResourceMetrics`].
//!
//! §157: Network, CPU and Storage are separate shards, each behind its
//! own lock; the only shared state is the atomic mode and the (lock-free)
//! pools. A storage backlog therefore cannot block CPU work, and there is
//! no giant scheduler lock.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::abuse::authorize_priority;
use crate::admission::{AdmissionResult, WorkPriority};
use crate::aging::{AgingPolicy, AgingQueue};
use crate::metrics::{Labels, ResourceMetrics};
use crate::peer_quota::TrustClass;
use crate::pressure::{shed, OverloadMode, ShedDecision};
use crate::queue::QueueCapacities;
use crate::reservation::{PermitBundle, ResourcePools};
use crate::types::ResourceKind;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShardKind {
    Network = 0,
    Cpu = 1,
    Storage = 2,
}

impl ShardKind {
    pub const ALL: [ShardKind; 3] = [Self::Network, Self::Cpu, Self::Storage];

    fn resource(self) -> ResourceKind {
        match self {
            Self::Network => ResourceKind::Network,
            Self::Cpu => ResourceKind::Cpu,
            Self::Storage => ResourceKind::Storage,
        }
    }
}

#[derive(Debug)]
pub struct Job<T> {
    pub payload: T,
    pub priority: WorkPriority,
    pub durable: bool,
    /// Everything this job needs *at the same time*.
    pub demand: Vec<(ResourceKind, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Queued,
    /// Durable work kept for later: shed by mode, or the shard is full.
    Deferred,
    /// Ephemeral work discarded.
    Dropped,
}

#[derive(Debug)]
#[must_use = "dropping `permits` releases the job's resources"]
pub struct Dispatched<T> {
    pub payload: T,
    pub priority: WorkPriority,
    pub shard: ShardKind,
    pub permits: PermitBundle,
    pub waited_millis: u64,
}

pub struct Scheduler<T> {
    shards: [Mutex<AgingQueue<Job<T>>>; 3],
    pools: ResourcePools,
    mode: AtomicU8,
    metrics: Arc<ResourceMetrics>,
}

fn mode_to_u8(m: OverloadMode) -> u8 {
    match m {
        OverloadMode::Normal => 0,
        OverloadMode::Shedding => 1,
        OverloadMode::Overload => 2,
        OverloadMode::CriticalOnly => 3,
    }
}

fn mode_from_u8(v: u8) -> OverloadMode {
    match v {
        0 => OverloadMode::Normal,
        1 => OverloadMode::Shedding,
        2 => OverloadMode::Overload,
        _ => OverloadMode::CriticalOnly, // unknown values fail safe
    }
}

impl<T> Scheduler<T> {
    pub fn new(
        capacities: QueueCapacities,
        aging: AgingPolicy,
        pools: ResourcePools,
        metrics: Arc<ResourceMetrics>,
    ) -> Self {
        Self {
            shards: [
                Mutex::new(AgingQueue::new(capacities, aging)),
                Mutex::new(AgingQueue::new(capacities, aging)),
                Mutex::new(AgingQueue::new(capacities, aging)),
            ],
            pools,
            mode: AtomicU8::new(0),
            metrics,
        }
    }

    pub fn set_mode(&self, m: OverloadMode) {
        self.mode.store(mode_to_u8(m), Ordering::Release);
    }

    pub fn mode(&self) -> OverloadMode {
        mode_from_u8(self.mode.load(Ordering::Acquire))
    }

    /// Ingress → admission → queue.
    pub fn submit(
        &self,
        shard: ShardKind,
        mut job: Job<T>,
        trust: TrustClass,
        now_millis: u64,
    ) -> SubmitOutcome {
        // A peer-claimed priority is clamped to what its trust allows
        // before it can influence queue position or shedding.
        job.priority = authorize_priority(trust, job.priority, false);
        let labels = Labels {
            trust,
            priority: job.priority,
            kind: shard.resource(),
        };

        match shed(self.mode(), job.priority, job.durable) {
            ShedDecision::Admit => {}
            ShedDecision::Defer => {
                self.metrics.record_shed(labels);
                return SubmitOutcome::Deferred;
            }
            ShedDecision::Drop => {
                self.metrics.record_shed(labels);
                return SubmitOutcome::Dropped;
            }
        }
        let (priority, durable) = (job.priority, job.durable);
        match lock(&self.shards[shard as usize]).enqueue(priority, job, durable, now_millis) {
            AdmissionResult::Accepted => SubmitOutcome::Queued,
            AdmissionResult::Deferred(_) => {
                self.metrics.record_deferred(labels);
                SubmitOutcome::Deferred
            }
            AdmissionResult::Dropped(_) | AdmissionResult::Rejected(_) => {
                self.metrics.record_reject(labels);
                SubmitOutcome::Dropped
            }
        }
    }

    /// Fair scheduler → permit acquisition → worker. Returns the next
    /// job whose whole demand can be met right now, else `None` (the
    /// queue is untouched).
    pub fn dispatch(&self, shard: ShardKind, now_millis: u64) -> Option<Dispatched<T>> {
        let (job, permits, enqueued) = lock(&self.shards[shard as usize])
            .dequeue_with(now_millis, |job| self.pools.acquire(&job.demand, None).ok())?;
        let waited = now_millis.saturating_sub(enqueued);
        self.metrics.record_queue_wait(job.priority, waited);
        Some(Dispatched {
            payload: job.payload,
            priority: job.priority,
            shard,
            permits,
            waited_millis: waited,
        })
    }

    pub fn queued(&self, shard: ShardKind) -> usize {
        lock(&self.shards[shard as usize]).len()
    }

    pub fn total_queued(&self) -> usize {
        ShardKind::ALL.iter().map(|s| self.queued(*s)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    fn sched(pools: ResourcePools) -> (Scheduler<&'static str>, Arc<ResourceMetrics>) {
        let m = Arc::new(ResourceMetrics::new());
        let s = Scheduler::new(
            QueueCapacities::conservative_default(),
            AgingPolicy {
                step_millis: 100,
                floor: Interactive,
            },
            pools,
            Arc::clone(&m),
        );
        (s, m)
    }

    fn pools() -> ResourcePools {
        ResourcePools::new(&[
            (ResourceKind::Cpu, 2),
            (ResourceKind::Memory, 100),
            (ResourceKind::Storage, 1),
            (ResourceKind::Network, 4),
        ])
    }

    fn job(
        name: &'static str,
        p: WorkPriority,
        demand: &[(ResourceKind, u64)],
    ) -> Job<&'static str> {
        Job {
            payload: name,
            priority: p,
            durable: true,
            demand: demand.to_vec(),
        }
    }

    #[test]
    fn priority_order_within_a_shard() {
        let (s, _) = sched(pools());
        for (n, p) in [("bulk", Bulk), ("crit", Critical), ("normal", Normal)] {
            s.submit(
                ShardKind::Cpu,
                job(n, p, &[(ResourceKind::Cpu, 0)]),
                TrustClass::LocalOwnDevice,
                0,
            );
        }
        let order: Vec<_> =
            std::iter::from_fn(|| s.dispatch(ShardKind::Cpu, 0).map(|d| d.payload)).collect();
        assert_eq!(order, vec!["crit", "normal", "bulk"]);
    }

    #[test]
    fn a_job_is_dispatched_only_when_its_whole_demand_is_available() {
        let (s, _) = sched(pools());
        s.submit(
            ShardKind::Cpu,
            job(
                "needs-both",
                Normal,
                &[(ResourceKind::Cpu, 1), (ResourceKind::Memory, 80)],
            ),
            TrustClass::Known,
            0,
        );
        let first = s.dispatch(ShardKind::Cpu, 0).unwrap();
        assert_eq!(s.pools.in_use(ResourceKind::Memory), 80);
        s.submit(
            ShardKind::Cpu,
            job(
                "second",
                Normal,
                &[(ResourceKind::Cpu, 1), (ResourceKind::Memory, 80)],
            ),
            TrustClass::Known,
            0,
        );
        assert!(
            s.dispatch(ShardKind::Cpu, 0).is_none(),
            "memory short: not dispatched"
        );
        assert_eq!(
            s.pools.in_use(ResourceKind::Cpu),
            1,
            "the waiting job holds nothing (no partial acquisition)"
        );
        assert_eq!(s.queued(ShardKind::Cpu), 1);
        drop(first);
        assert_eq!(s.dispatch(ShardKind::Cpu, 1).unwrap().payload, "second");
    }

    #[test]
    fn a_blocked_job_keeps_its_place_and_age() {
        let (s, _) = sched(pools());
        s.submit(
            ShardKind::Cpu,
            job("big", Bulk, &[(ResourceKind::Memory, 90)]),
            TrustClass::Known,
            0,
        );
        let hold = s
            .pools
            .acquire(&[(ResourceKind::Memory, 50)], None)
            .unwrap();
        assert!(s.dispatch(ShardKind::Cpu, 5).is_none());
        s.submit(
            ShardKind::Cpu,
            job("fresh", Normal, &[(ResourceKind::Memory, 1)]),
            TrustClass::Known,
            1_000,
        );
        drop(hold);
        // "big" waited since t=0, so it has aged above fresh Normal
        let d = s.dispatch(ShardKind::Cpu, 1_000).unwrap();
        assert_eq!((d.payload, d.waited_millis), ("big", 1_000));
    }

    #[test]
    fn shards_are_independent_a_stuck_storage_shard_does_not_block_cpu() {
        let (s, _) = sched(pools());
        let _squat = s
            .pools
            .acquire(&[(ResourceKind::Storage, 1)], None)
            .unwrap();
        s.submit(
            ShardKind::Storage,
            job("disk", Critical, &[(ResourceKind::Storage, 1)]),
            TrustClass::LocalOwnDevice,
            0,
        );
        s.submit(
            ShardKind::Cpu,
            job("hash", Bulk, &[(ResourceKind::Cpu, 1)]),
            TrustClass::Known,
            0,
        );
        assert!(s.dispatch(ShardKind::Storage, 0).is_none());
        assert_eq!(s.dispatch(ShardKind::Cpu, 0).unwrap().payload, "hash");
    }

    #[test]
    fn untrusted_peers_cannot_jump_the_queue_by_claiming_critical() {
        let (s, _) = sched(pools());
        s.submit(
            ShardKind::Network,
            job("attacker", Critical, &[]),
            TrustClass::Unknown,
            0,
        );
        s.submit(
            ShardKind::Network,
            job("friend", Interactive, &[]),
            TrustClass::Known,
            1,
        );
        assert_eq!(s.dispatch(ShardKind::Network, 1).unwrap().payload, "friend");
        assert_eq!(
            s.dispatch(ShardKind::Network, 1).unwrap().priority,
            Normal,
            "clamped from Critical"
        );
    }

    #[test]
    fn overload_mode_sheds_by_priority_deferring_durable_and_dropping_ephemeral() {
        let (s, m) = sched(pools());
        s.set_mode(OverloadMode::Overload);
        assert_eq!(
            s.submit(
                ShardKind::Network,
                job("bulk-durable", Bulk, &[]),
                TrustClass::Known,
                0
            ),
            SubmitOutcome::Deferred
        );
        let mut eph = job("bulk-ephemeral", Bulk, &[]);
        eph.durable = false;
        assert_eq!(
            s.submit(ShardKind::Network, eph, TrustClass::Known, 0),
            SubmitOutcome::Dropped
        );
        assert_eq!(
            s.submit(
                ShardKind::Network,
                job("inter", Interactive, &[]),
                TrustClass::Known,
                0
            ),
            SubmitOutcome::Queued
        );
        assert_eq!(s.total_queued(), 1);
        let l = Labels {
            trust: TrustClass::Known,
            priority: Bulk,
            kind: ResourceKind::Network,
        };
        assert_eq!(m.counters(l).shed, 2, "feedback recorded");
        s.set_mode(OverloadMode::CriticalOnly);
        assert_eq!(
            s.submit(
                ShardKind::Network,
                job("inter2", Interactive, &[]),
                TrustClass::Known,
                0
            ),
            SubmitOutcome::Deferred
        );
        assert_eq!(
            s.submit(
                ShardKind::Network,
                job("ctl", Control, &[]),
                TrustClass::LocalOwnDevice,
                0
            ),
            SubmitOutcome::Queued
        );
    }

    #[test]
    fn queue_wait_is_recorded_as_feedback() {
        let (s, m) = sched(pools());
        s.submit(ShardKind::Cpu, job("j", Normal, &[]), TrustClass::Known, 0);
        let d = s.dispatch(ShardKind::Cpu, 700).unwrap();
        assert_eq!(d.waited_millis, 700);
        assert_eq!(m.queue_wait(Normal).count(), 1);
        assert_eq!(m.queue_wait(Normal).max_ms(), 700);
    }

    #[test]
    fn a_full_shard_defers_durable_and_rejects_ephemeral_work() {
        let m = Arc::new(ResourceMetrics::new());
        let caps = QueueCapacities {
            critical: 1,
            control: 1,
            interactive: 1,
            normal: 1,
            bulk: 1,
            background: 1,
        };
        let s: Scheduler<&str> =
            Scheduler::new(caps, AgingPolicy::default(), pools(), Arc::clone(&m));
        assert_eq!(
            s.submit(ShardKind::Cpu, job("a", Normal, &[]), TrustClass::Known, 0),
            SubmitOutcome::Queued
        );
        assert_eq!(
            s.submit(ShardKind::Cpu, job("b", Normal, &[]), TrustClass::Known, 0),
            SubmitOutcome::Deferred
        );
        let mut e = job("c", Normal, &[]);
        e.durable = false;
        assert_eq!(
            s.submit(ShardKind::Cpu, e, TrustClass::Known, 0),
            SubmitOutcome::Dropped
        );
        let l = Labels {
            trust: TrustClass::Known,
            priority: Normal,
            kind: ResourceKind::Cpu,
        };
        assert_eq!(
            (m.counters(l).deferred, m.counters(l).admission_rejects),
            (1, 1)
        );
        // other shards have their own capacity
        assert_eq!(
            s.submit(
                ShardKind::Network,
                job("d", Normal, &[]),
                TrustClass::Known,
                0
            ),
            SubmitOutcome::Queued
        );
    }

    #[test]
    fn mode_round_trips_and_unknown_raw_values_fail_safe() {
        let (s, _) = sched(pools());
        for m in [
            OverloadMode::Normal,
            OverloadMode::Shedding,
            OverloadMode::Overload,
            OverloadMode::CriticalOnly,
        ] {
            s.set_mode(m);
            assert_eq!(s.mode(), m);
        }
        assert_eq!(mode_from_u8(250), OverloadMode::CriticalOnly);
    }

    #[test]
    fn concurrent_producers_and_consumers_conserve_jobs_and_release_everything() {
        let p = ResourcePools::new(&[(ResourceKind::Cpu, 3), (ResourceKind::Memory, 1_000)]);
        let m = Arc::new(ResourceMetrics::new());
        let s: Arc<Scheduler<u32>> = Arc::new(Scheduler::new(
            QueueCapacities {
                critical: 10_000,
                control: 10_000,
                interactive: 10_000,
                normal: 10_000,
                bulk: 10_000,
                background: 10_000,
            },
            AgingPolicy::default(),
            p.clone(),
            m,
        ));
        let producers: Vec<_> = (0..4u32)
            .map(|t| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || {
                    for i in 0..500u32 {
                        let sh = ShardKind::ALL[(i % 3) as usize];
                        let j = Job {
                            payload: t * 1_000 + i,
                            priority: Normal,
                            durable: true,
                            demand: vec![(ResourceKind::Cpu, 1), (ResourceKind::Memory, 100)],
                        };
                        assert_eq!(s.submit(sh, j, TrustClass::Known, 0), SubmitOutcome::Queued);
                    }
                })
            })
            .collect();
        producers.into_iter().for_each(|h| h.join().unwrap());
        let done = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let consumers: Vec<_> = (0..3)
            .map(|c| {
                let (s, done, p) = (Arc::clone(&s), Arc::clone(&done), p.clone());
                std::thread::spawn(move || {
                    let sh = ShardKind::ALL[c];
                    let mut idle = 0;
                    while idle < 20_000 {
                        match s.dispatch(sh, 1) {
                            Some(d) => {
                                assert!(
                                    p.in_use(ResourceKind::Cpu) <= 3
                                        && p.in_use(ResourceKind::Memory) <= 1_000
                                );
                                done.fetch_add(1, Ordering::Relaxed);
                                drop(d);
                                idle = 0;
                            }
                            None => {
                                if s.queued(sh) == 0 {
                                    break;
                                }
                                idle += 1;
                                std::thread::yield_now();
                            }
                        }
                    }
                })
            })
            .collect();
        consumers.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(
            done.load(Ordering::Relaxed),
            2_000,
            "every job ran exactly once"
        );
        assert_eq!(s.total_queued(), 0);
        assert_eq!(
            (p.in_use(ResourceKind::Cpu), p.in_use(ResourceKind::Memory)),
            (0, 0)
        );
    }
}
