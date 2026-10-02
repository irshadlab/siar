//! §161 task registry, §162 no fire-and-forget by default.
//!
//! Every long-lived task registers and holds a [`TaskGuard`]; the guard
//! deregisters on drop (including panic unwind), so the registry can't
//! drift from reality. The registry is *bounded*: once `max_tasks` are
//! live, `register` refuses — an unbounded spawn loop becomes a visible
//! error instead of a leak.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::admission::WorkPriority;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInfo {
    pub id: u64,
    pub owner: &'static str,
    pub priority: WorkPriority,
    pub age_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("task registry full ({0} live tasks); refusing to track another")]
pub struct RegistryFull(pub usize);

#[derive(Debug)]
struct Entry {
    owner: &'static str,
    priority: WorkPriority,
    started_at: u64,
}

#[derive(Debug)]
struct Inner {
    max_tasks: usize,
    state: Mutex<(u64, HashMap<u64, Entry>)>, // (next id, live tasks)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone)]
pub struct TaskRegistry(Arc<Inner>);

#[derive(Debug)]
#[must_use = "dropping the guard deregisters the task"]
pub struct TaskGuard {
    inner: Arc<Inner>,
    id: u64,
}

impl TaskRegistry {
    pub fn new(max_tasks: usize) -> Self {
        Self(Arc::new(Inner {
            max_tasks: max_tasks.max(1),
            state: Mutex::new((0, HashMap::new())),
        }))
    }

    pub fn register(
        &self,
        owner: &'static str,
        priority: WorkPriority,
        now_millis: u64,
    ) -> Result<TaskGuard, RegistryFull> {
        let mut st = lock(&self.0.state);
        if st.1.len() >= self.0.max_tasks {
            return Err(RegistryFull(self.0.max_tasks));
        }
        let id = st.0;
        st.0 += 1;
        st.1.insert(
            id,
            Entry {
                owner,
                priority,
                started_at: now_millis,
            },
        );
        drop(st);
        Ok(TaskGuard {
            inner: Arc::clone(&self.0),
            id,
        })
    }

    pub fn active(&self) -> usize {
        lock(&self.0.state).1.len()
    }

    pub fn active_for(&self, owner: &str) -> usize {
        lock(&self.0.state)
            .1
            .values()
            .filter(|e| e.owner == owner)
            .count()
    }

    /// Diagnostics snapshot, oldest first.
    pub fn snapshot(&self, now_millis: u64) -> Vec<TaskInfo> {
        let st = lock(&self.0.state);
        let mut v: Vec<TaskInfo> =
            st.1.iter()
                .map(|(id, e)| TaskInfo {
                    id: *id,
                    owner: e.owner,
                    priority: e.priority,
                    age_millis: now_millis.saturating_sub(e.started_at),
                })
                .collect();
        v.sort_by(|a, b| b.age_millis.cmp(&a.age_millis).then(a.id.cmp(&b.id)));
        v
    }

    /// Tasks older than `threshold` — candidates for "stuck" diagnostics.
    pub fn older_than(&self, now_millis: u64, threshold_millis: u64) -> Vec<TaskInfo> {
        self.snapshot(now_millis)
            .into_iter()
            .filter(|t| t.age_millis > threshold_millis)
            .collect()
    }
}

impl TaskGuard {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        lock(&self.inner.state).1.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    #[test]
    fn guard_drop_deregisters_even_on_panic() {
        let r = TaskRegistry::new(8);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = r.register("dtn", Normal, 0).unwrap();
            assert_eq!(r.active(), 1);
            panic!("worker died");
        }));
        assert!(res.is_err());
        assert_eq!(r.active(), 0);
    }

    #[test]
    fn registry_is_bounded_so_unbounded_spawn_is_an_error() {
        let r = TaskRegistry::new(2);
        let _a = r.register("x", Normal, 0).unwrap();
        let _b = r.register("x", Normal, 0).unwrap();
        assert_eq!(r.register("x", Normal, 0).unwrap_err(), RegistryFull(2));
    }

    #[test]
    fn snapshot_reports_owner_priority_age_oldest_first() {
        let r = TaskRegistry::new(8);
        let _a = r.register("routing", Control, 100).unwrap();
        let _b = r.register("files", Bulk, 50).unwrap();
        let s = r.snapshot(200);
        assert_eq!(s[0].owner, "files");
        assert_eq!(s[0].age_millis, 150);
        assert_eq!(s[1].priority, Control);
        assert_eq!(r.active_for("files"), 1);
        assert_eq!(r.older_than(200, 120).len(), 1);
    }

    #[test]
    fn ids_are_never_reused() {
        let r = TaskRegistry::new(8);
        let a = r.register("x", Normal, 0).unwrap();
        let id = a.id();
        drop(a);
        assert_ne!(r.register("x", Normal, 0).unwrap().id(), id);
    }

    #[test]
    fn clock_going_backwards_gives_zero_age_not_a_wrap() {
        let r = TaskRegistry::new(8);
        let _a = r.register("x", Normal, 1_000).unwrap();
        assert_eq!(r.snapshot(10)[0].age_millis, 0);
    }
}
