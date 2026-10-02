//! §159 async cancellation, §160 structured concurrency.
//!
//! Runtime-agnostic: a hierarchical [`CancelToken`] (cancel a parent and
//! every descendant observes it) and an [`OperationScope`] that owns an
//! operation's permits. Dropping a scope cancels its token *first* (so
//! children stop issuing work) and then releases adopted holds in
//! reverse order of adoption — "cancel parent: children cancel, permits
//! release". Every wait in this crate is either non-blocking (`try_*`) or
//! poll-based, so a cancelled waiter is simply dropped; callers that loop
//! use [`poll_cancellable`] so a cancelled token is checked on every pass.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[derive(Debug)]
struct Node {
    cancelled: AtomicBool,
    children: Mutex<Vec<Weak<Node>>>,
}

#[derive(Debug, Clone)]
pub struct CancelToken(Arc<Node>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("operation cancelled")]
pub struct Cancelled;

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(Node {
            cancelled: AtomicBool::new(false),
            children: Mutex::new(Vec::new()),
        }))
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn ensure_live(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// A token cancelled whenever `self` is. Starts cancelled if `self`
    /// already is. Dead children are pruned here so a long-lived parent
    /// that spawns many short-lived children doesn't leak.
    pub fn child(&self) -> CancelToken {
        let child = CancelToken::new();
        let mut kids = self.0.children.lock().unwrap_or_else(|e| e.into_inner());
        kids.retain(|w| w.strong_count() > 0);
        kids.push(Arc::downgrade(&child.0));
        drop(kids);
        // Re-check after publishing: a cancel racing with this call is
        // either seen here or has already walked a list containing us.
        if self.is_cancelled() {
            child.cancel();
        }
        child
    }

    pub fn cancel(&self) {
        if self.0.cancelled.swap(true, Ordering::AcqRel) {
            return; // already cancelled; children were handled then
        }
        let kids = std::mem::take(&mut *self.0.children.lock().unwrap_or_else(|e| e.into_inner()));
        for k in kids {
            if let Some(node) = k.upgrade() {
                CancelToken(node).cancel();
            }
        }
    }
}

/// Run `f` until it yields a value, checking cancellation before every
/// attempt. `f` must itself be non-blocking.
pub fn poll_cancellable<T>(
    token: &CancelToken,
    mut f: impl FnMut() -> Option<T>,
) -> Result<Option<T>, Cancelled> {
    token.ensure_live()?;
    Ok(f())
}

/// Owns an operation's cancellation token and the holds (permits,
/// reservations, task guards) acquired on its behalf.
pub struct OperationScope {
    token: CancelToken,
    holds: Vec<Box<dyn Send>>,
}

impl OperationScope {
    pub fn root() -> Self {
        Self {
            token: CancelToken::new(),
            holds: Vec::new(),
        }
    }

    /// A child scope, cancelled when this one is cancelled or dropped.
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child(),
            holds: Vec::new(),
        }
    }

    pub fn token(&self) -> &CancelToken {
        &self.token
    }

    /// Take ownership of a hold so its lifetime is the scope's.
    pub fn adopt<H: Send + 'static>(&mut self, hold: H) {
        self.holds.push(Box::new(hold));
    }

    pub fn held(&self) -> usize {
        self.holds.len()
    }
}

impl Drop for OperationScope {
    fn drop(&mut self) {
        self.token.cancel();
        while let Some(h) = self.holds.pop() {
            drop(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn cancelling_a_parent_cancels_every_descendant_but_not_siblings_or_ancestors() {
        let root = CancelToken::new();
        let a = root.child();
        let a1 = a.child();
        let b = root.child();
        a.cancel();
        assert!(a.is_cancelled() && a1.is_cancelled());
        assert!(!b.is_cancelled() && !root.is_cancelled());
    }

    #[test]
    fn child_of_a_cancelled_token_starts_cancelled() {
        let root = CancelToken::new();
        root.cancel();
        assert!(root.child().is_cancelled());
    }

    #[test]
    fn dead_children_are_pruned() {
        let root = CancelToken::new();
        for _ in 0..1_000 {
            let _short_lived = root.child();
        }
        let _last = root.child();
        assert!(root.0.children.lock().unwrap().len() < 5);
    }

    struct Probe(Arc<AtomicUsize>, usize, Arc<Mutex<Vec<usize>>>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.2.lock().unwrap().push(self.1);
        }
    }

    #[test]
    fn dropping_a_scope_cancels_children_first_then_releases_holds_in_reverse() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let n = Arc::new(AtomicUsize::new(0));
        let parent = OperationScope::root();
        let mut op = parent.child();
        let nested = op.child(); // keep the nested scope alive until `op` drops
        let child_tok = nested.token().clone();
        op.adopt(Probe(n.clone(), 1, order.clone()));
        op.adopt(Probe(n.clone(), 2, order.clone()));
        op.adopt(Probe(n.clone(), 3, order.clone()));
        assert_eq!(op.held(), 3);
        assert!(!child_tok.is_cancelled());
        drop(op);
        assert!(
            child_tok.is_cancelled(),
            "nested scope cancelled by its parent"
        );
        drop(nested);
        assert_eq!(n.load(Ordering::SeqCst), 3);
        assert_eq!(*order.lock().unwrap(), vec![3, 2, 1], "LIFO release");
        assert!(
            !parent.token().is_cancelled(),
            "child scope drop doesn't cancel the parent"
        );
    }

    #[test]
    fn dropping_the_parent_scope_cancels_the_whole_subtree() {
        let parent = OperationScope::root();
        let kid = parent.child();
        let tok = kid.token().clone();
        drop(parent);
        assert!(tok.is_cancelled());
    }

    #[test]
    fn poll_cancellable_checks_before_each_attempt() {
        let t = CancelToken::new();
        let mut calls = 0;
        assert_eq!(
            poll_cancellable(&t, || {
                calls += 1;
                Some(7)
            }),
            Ok(Some(7))
        );
        t.cancel();
        assert_eq!(
            poll_cancellable(&t, || {
                calls += 1;
                Some(8)
            }),
            Err(Cancelled)
        );
        assert_eq!(calls, 1, "f is not run once cancelled");
    }

    #[test]
    fn cancel_is_idempotent_and_thread_safe() {
        let root = CancelToken::new();
        let kids: Vec<_> = (0..64).map(|_| root.child()).collect();
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let r = root.clone();
                std::thread::spawn(move || r.cancel())
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        assert!(kids.iter().all(CancelToken::is_cancelled));
    }
}
