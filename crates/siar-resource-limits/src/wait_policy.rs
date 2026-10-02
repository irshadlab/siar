//! §111 timeout policy, §112 wait vs fail.
//!
//! Waiting for a resource is policy per class: a video frame that cannot
//! be admitted in milliseconds is worthless (fail fast, fall back); a
//! text send waits briefly then persists; a background file waits
//! durably, indefinitely.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitClass {
    /// Video/audio frames, call setup.
    Realtime,
    /// A user-initiated send.
    Interactive,
    /// Background file / sync work.
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitPolicy {
    pub realtime_ms: u64,
    pub interactive_ms: u64,
}

impl Default for WaitPolicy {
    fn default() -> Self {
        Self {
            realtime_ms: 20,
            interactive_ms: 3_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitVerdict {
    KeepWaiting,
    /// Gave up; take the cheaper fallback path (lower bitrate, audio-only,
    /// different route) rather than reporting an error.
    FailWithFallback,
    /// Gave up waiting *now* but keep the work: persist and retry later.
    DeferDurably,
    /// Gave up and the work is not worth keeping.
    Fail,
}

impl WaitPolicy {
    /// How long a class may wait; `None` = no timeout (durable wait).
    pub fn timeout_ms(&self, class: WaitClass) -> Option<u64> {
        match class {
            WaitClass::Realtime => Some(self.realtime_ms),
            WaitClass::Interactive => Some(self.interactive_ms),
            // Background work waits without a timeout: durable work is
            // deferred, ephemeral work fails at the first shortfall.
            WaitClass::Background => None,
        }
    }

    pub fn deadline(&self, class: WaitClass, now_millis: u64) -> Option<u64> {
        self.timeout_ms(class).map(|t| now_millis.saturating_add(t))
    }

    /// Decide what to do after `waited_ms` without the resource.
    pub fn verdict(&self, class: WaitClass, durable: bool, waited_ms: u64) -> WaitVerdict {
        match class {
            WaitClass::Realtime => {
                if waited_ms < self.realtime_ms {
                    WaitVerdict::KeepWaiting
                } else {
                    WaitVerdict::FailWithFallback
                }
            }
            WaitClass::Interactive => {
                if waited_ms < self.interactive_ms {
                    WaitVerdict::KeepWaiting
                } else if durable {
                    WaitVerdict::DeferDurably
                } else {
                    WaitVerdict::Fail
                }
            }
            WaitClass::Background => {
                if durable {
                    WaitVerdict::DeferDurably
                } else {
                    WaitVerdict::Fail
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_fails_fast_with_fallback() {
        let p = WaitPolicy::default();
        assert_eq!(
            p.verdict(WaitClass::Realtime, false, 5),
            WaitVerdict::KeepWaiting
        );
        assert_eq!(
            p.verdict(WaitClass::Realtime, false, 20),
            WaitVerdict::FailWithFallback
        );
        assert_eq!(
            p.verdict(WaitClass::Realtime, true, 20),
            WaitVerdict::FailWithFallback
        );
    }

    #[test]
    fn interactive_waits_then_persists_if_durable() {
        let p = WaitPolicy::default();
        assert_eq!(
            p.verdict(WaitClass::Interactive, true, 2_999),
            WaitVerdict::KeepWaiting
        );
        assert_eq!(
            p.verdict(WaitClass::Interactive, true, 3_000),
            WaitVerdict::DeferDurably
        );
        assert_eq!(
            p.verdict(WaitClass::Interactive, false, 3_000),
            WaitVerdict::Fail
        );
    }

    #[test]
    fn durable_background_never_times_out_it_defers() {
        let p = WaitPolicy::default();
        assert_eq!(
            p.verdict(WaitClass::Background, true, 0),
            WaitVerdict::DeferDurably
        );
        assert_eq!(p.timeout_ms(WaitClass::Background), None);
        assert_eq!(p.deadline(WaitClass::Background, 10), None);
        assert_eq!(
            p.verdict(WaitClass::Background, false, 0),
            WaitVerdict::Fail
        );
    }

    #[test]
    fn deadline_saturates() {
        let p = WaitPolicy::default();
        assert_eq!(p.deadline(WaitClass::Interactive, u64::MAX), Some(u64::MAX));
        assert_eq!(p.deadline(WaitClass::Realtime, 100), Some(120));
    }
}
