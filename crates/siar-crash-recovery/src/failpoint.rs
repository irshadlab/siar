//! §121 "Failpoint Framework".
//!
//! ```ignore
//! fn commit_message() -> Result<(), siar_crash_recovery::failpoint::Triggered> {
//!     db.commit();
//!     siar_crash_recovery::failpoint!("after_message_commit");
//!     send();
//!     Ok(())
//! }
//! ```
//!
//! Compiled in only for tests, debug builds, or the
//! `crash-injection` feature (§121: "enabled only in tests/debug
//! builds"). In a release build [`hit`] is an inlined `Ok(())` and the
//! macro costs nothing. The registry is **thread-local**, so parallel
//! tests arming the same name cannot interfere with each other.

use std::fmt;

/// §121's two example names, plus a stable home for others.
pub const AFTER_MESSAGE_COMMIT: &str = "after_message_commit";
pub const BEFORE_BLOB_RENAME: &str = "before_blob_rename";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailAction {
    /// Simulates process death at this point: the caller must abandon
    /// all in-memory state.
    Crash,
    /// A recoverable error return (I/O error, busy, …).
    InjectedError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("failpoint `{name}` triggered ({action:?})")]
pub struct Triggered {
    pub name: &'static str,
    pub action: FailAction,
}

/// `failpoint!("name")` — propagates a [`Triggered`] with `?`, so the
/// enclosing function's error type must implement
/// `From<Triggered>`.
#[macro_export]
macro_rules! failpoint {
    ($name:expr) => {
        $crate::failpoint::hit($name)?
    };
}

#[cfg(any(test, debug_assertions, feature = "crash-injection"))]
mod enabled {
    use super::{FailAction, Triggered};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Copy)]
    struct Armed {
        action: FailAction,
        /// Fire on this (1-based) hit.
        nth: u32,
        seen: u32,
        fired: bool,
    }

    thread_local! {
        static REGISTRY: RefCell<HashMap<&'static str, Armed>> = RefCell::new(HashMap::new());
    }

    /// Disarms its failpoint when dropped, so a panicking test cannot
    /// leave one armed for the next test on the same thread.
    #[derive(Debug)]
    #[must_use = "the failpoint is disarmed as soon as the guard is dropped"]
    pub struct ArmedGuard {
        name: &'static str,
    }

    impl Drop for ArmedGuard {
        fn drop(&mut self) {
            REGISTRY.with(|r| {
                r.borrow_mut().remove(self.name);
            });
        }
    }

    /// Arms `name` to trigger `action` on its `nth` (1-based) hit.
    pub fn arm(name: &'static str, action: FailAction, nth: u32) -> ArmedGuard {
        REGISTRY.with(|r| {
            r.borrow_mut().insert(
                name,
                Armed {
                    action,
                    nth: nth.max(1),
                    seen: 0,
                    fired: false,
                },
            );
        });
        ArmedGuard { name }
    }

    pub fn disarm_all() {
        REGISTRY.with(|r| r.borrow_mut().clear());
    }

    /// Fires at most once per arming.
    pub fn hit(name: &'static str) -> Result<(), Triggered> {
        REGISTRY.with(|r| {
            let mut r = r.borrow_mut();
            let Some(a) = r.get_mut(name) else {
                return Ok(());
            };
            if a.fired {
                return Ok(());
            }
            a.seen += 1;
            if a.seen == a.nth {
                a.fired = true;
                Err(Triggered {
                    name,
                    action: a.action,
                })
            } else {
                Ok(())
            }
        })
    }
}

#[cfg(any(test, debug_assertions, feature = "crash-injection"))]
pub use enabled::{arm, disarm_all, hit, ArmedGuard};

/// Release build: no registry, no cost.
#[cfg(not(any(test, debug_assertions, feature = "crash-injection")))]
#[inline(always)]
pub const fn hit(_name: &'static str) -> Result<(), Triggered> {
    Ok(())
}

impl fmt::Display for FailAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FailAction::Crash => "crash",
            FailAction::InjectedError => "injected-error",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum E {
        Fp(Triggered),
    }
    impl From<Triggered> for E {
        fn from(t: Triggered) -> Self {
            E::Fp(t)
        }
    }

    fn op(commits: &mut u32, sends: &mut u32) -> Result<(), E> {
        *commits += 1;
        crate::failpoint!("after_message_commit");
        *sends += 1;
        Ok(())
    }

    #[test]
    fn unarmed_failpoints_are_invisible() {
        let (mut c, mut s) = (0, 0);
        assert_eq!(op(&mut c, &mut s), Ok(()));
        assert_eq!((c, s), (1, 1));
    }

    #[test]
    fn armed_failpoint_aborts_between_the_two_steps() {
        let _g = arm(AFTER_MESSAGE_COMMIT, FailAction::Crash, 1);
        let (mut c, mut s) = (0, 0);
        let r = op(&mut c, &mut s);
        assert_eq!(
            r,
            Err(E::Fp(Triggered {
                name: AFTER_MESSAGE_COMMIT,
                action: FailAction::Crash
            }))
        );
        assert_eq!((c, s), (1, 0), "committed, but never sent");
    }

    #[test]
    fn nth_hit_and_fire_once() {
        let _g = arm(BEFORE_BLOB_RENAME, FailAction::InjectedError, 2);
        assert!(hit(BEFORE_BLOB_RENAME).is_ok());
        assert!(hit(BEFORE_BLOB_RENAME).is_err());
        assert!(hit(BEFORE_BLOB_RENAME).is_ok(), "fires once per arming");
    }

    #[test]
    fn guard_disarms_on_drop() {
        {
            let _g = arm("temp_point", FailAction::Crash, 1);
        }
        assert!(hit("temp_point").is_ok());
    }

    #[test]
    fn arming_is_per_thread_so_parallel_tests_cannot_collide() {
        let _g = arm("shared_name", FailAction::Crash, 1);
        let other = std::thread::spawn(|| hit("shared_name")).join().unwrap();
        assert!(other.is_ok(), "other thread's registry is independent");
        assert!(hit("shared_name").is_err());
    }

    #[test]
    fn disarm_all_clears_everything() {
        let g1 = arm("a", FailAction::Crash, 1);
        let g2 = arm("b", FailAction::Crash, 1);
        disarm_all();
        assert!(hit("a").is_ok() && hit("b").is_ok());
        drop((g1, g2));
    }
}
