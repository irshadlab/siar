//! §98 fair semaphores.
//!
//! A plain semaphore lets a hot task release and immediately re-acquire
//! forever while others wait. This one is strictly FIFO: a new request
//! may take capacity immediately only if *nobody is queued*; otherwise it
//! joins the queue, and only the head of the queue may be granted.
//! Waiters are RAII — dropping one (a cancelled future) leaves the queue,
//! so an abandoned waiter can never block the line.
//!
//! The API is poll-based and runtime-agnostic; an async wrapper turns
//! [`FairSemaphore::head_grantable`] into a wake-up.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug)]
struct State {
    available: u64,
    next_id: u64,
    queue: VecDeque<(u64, u64)>, // (ticket id, amount)
}

#[derive(Debug)]
struct Inner {
    capacity: u64,
    state: Mutex<State>,
}

#[derive(Debug, Clone)]
pub struct FairSemaphore(Arc<Inner>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FairError {
    #[error("request of {requested} can never be satisfied (capacity {capacity})")]
    TooLarge { requested: u64, capacity: u64 },
    #[error("others are waiting; join the queue")]
    WouldWait,
}

#[derive(Debug)]
#[must_use = "dropping the permit returns capacity"]
pub struct FairPermit {
    inner: Arc<Inner>,
    amount: u64,
}

#[derive(Debug)]
pub struct Waiter {
    inner: Arc<Inner>,
    id: u64,
    amount: u64,
    done: bool,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FairSemaphore {
    pub fn new(capacity: u64) -> Self {
        Self(Arc::new(Inner {
            capacity,
            state: Mutex::new(State {
                available: capacity,
                next_id: 0,
                queue: VecDeque::new(),
            }),
        }))
    }

    fn check(&self, n: u64) -> Result<(), FairError> {
        if n > self.0.capacity {
            Err(FairError::TooLarge {
                requested: n,
                capacity: self.0.capacity,
            })
        } else {
            Ok(())
        }
    }

    /// Immediate acquire; only succeeds if nobody is queued ahead.
    pub fn try_acquire(&self, n: u64) -> Result<FairPermit, FairError> {
        self.check(n)?;
        let mut st = lock(&self.0.state);
        if st.queue.is_empty() && st.available >= n {
            st.available -= n;
            return Ok(FairPermit {
                inner: Arc::clone(&self.0),
                amount: n,
            });
        }
        Err(FairError::WouldWait)
    }

    /// Join the line. (A `TooLarge` request is refused here, not queued —
    /// it would block everyone behind it forever.)
    pub fn wait(&self, n: u64) -> Result<Waiter, FairError> {
        self.check(n)?;
        let mut st = lock(&self.0.state);
        let id = st.next_id;
        st.next_id += 1;
        st.queue.push_back((id, n));
        Ok(Waiter {
            inner: Arc::clone(&self.0),
            id,
            amount: n,
            done: false,
        })
    }

    /// True if the waiter at the head could be granted right now (the
    /// signal an async wrapper uses to wake exactly one task).
    pub fn head_grantable(&self) -> bool {
        let st = lock(&self.0.state);
        st.queue.front().is_some_and(|&(_, n)| st.available >= n)
    }

    pub fn available(&self) -> u64 {
        lock(&self.0.state).available
    }

    pub fn queued(&self) -> usize {
        lock(&self.0.state).queue.len()
    }
}

impl Waiter {
    /// `Some(permit)` once this waiter is at the head and capacity exists.
    pub fn poll(&mut self) -> Option<FairPermit> {
        if self.done {
            return None;
        }
        let mut st = lock(&self.inner.state);
        let at_head = st.queue.front().is_some_and(|&(id, _)| id == self.id);
        if at_head && st.available >= self.amount {
            st.queue.pop_front();
            st.available -= self.amount;
            self.done = true;
            return Some(FairPermit {
                inner: Arc::clone(&self.inner),
                amount: self.amount,
            });
        }
        None
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.done {
            let mut st = lock(&self.inner.state);
            st.queue.retain(|&(id, _)| id != self.id);
        }
    }
}

impl Drop for FairPermit {
    fn drop(&mut self) {
        let mut st = lock(&self.inner.state);
        st.available = (st.available + self.amount).min(self.inner.capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_releasing_task_cannot_jump_the_queue() {
        let s = FairSemaphore::new(1);
        let held = s.try_acquire(1).unwrap();
        let mut w = s.wait(1).unwrap();
        assert!(w.poll().is_none());
        drop(held);
        // The hot task tries to re-acquire immediately: must be refused.
        assert_eq!(s.try_acquire(1).unwrap_err(), FairError::WouldWait);
        assert!(s.head_grantable());
        let p = w.poll().expect("waiter gets it");
        assert_eq!(s.available(), 0);
        drop(p);
        assert!(s.try_acquire(1).is_ok(), "queue is empty again");
    }

    #[test]
    fn strict_fifo_even_when_a_later_small_request_would_fit() {
        let s = FairSemaphore::new(4);
        let a = s.try_acquire(3).unwrap(); // 1 left
        let mut big = s.wait(3).unwrap();
        let mut small = s.wait(1).unwrap();
        assert!(small.poll().is_none(), "not at head, despite 1 being free");
        drop(a);
        let _p1 = big.poll().unwrap();
        let _p2 = small.poll().unwrap();
    }

    #[test]
    fn dropping_a_waiter_unblocks_the_line() {
        let s = FairSemaphore::new(2);
        let _hold = s.try_acquire(2).unwrap();
        let abandoned = s.wait(2).unwrap();
        let mut next = s.wait(1).unwrap();
        assert_eq!(s.queued(), 2);
        drop(abandoned);
        assert_eq!(s.queued(), 1);
        drop(_hold);
        assert!(next.poll().is_some());
    }

    #[test]
    fn impossible_requests_are_refused_not_queued() {
        let s = FairSemaphore::new(2);
        assert_eq!(
            s.wait(3).unwrap_err(),
            FairError::TooLarge {
                requested: 3,
                capacity: 2
            }
        );
        assert_eq!(s.queued(), 0);
        assert!(s.try_acquire(3).is_err());
    }

    #[test]
    fn a_polled_and_granted_waiter_does_not_double_release_on_drop() {
        let s = FairSemaphore::new(1);
        let mut w = s.wait(1).unwrap();
        let p = w.poll().unwrap();
        drop(w); // done == true: no queue removal
        assert_eq!(s.available(), 0);
        drop(p);
        assert_eq!(s.available(), 1);
    }

    #[test]
    fn contended_threads_all_make_progress() {
        let s = FairSemaphore::new(1);
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let s = s.clone();
                std::thread::spawn(move || {
                    let mut got = 0u32;
                    for _ in 0..200 {
                        let mut w = s.wait(1).unwrap();
                        loop {
                            if let Some(p) = w.poll() {
                                got += 1;
                                drop(p);
                                break;
                            }
                            std::thread::yield_now();
                        }
                    }
                    got
                })
            })
            .collect();
        for h in hs {
            assert_eq!(h.join().unwrap(), 200, "no thread starved");
        }
        assert_eq!((s.available(), s.queued()), (1, 0));
    }
}
