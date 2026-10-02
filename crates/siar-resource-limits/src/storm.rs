//! §69 connection storm protection, §129 unknown-peer admission, §131
//! handshake budget.
//!
//! Each source class has its *own* token bucket, drawn before the shared
//! global one. A flood from unknown peers can therefore only drain the
//! unknown bucket; trusted and known peers keep their guaranteed rate.
//! (A single shared bucket lets whoever sends fastest starve everyone.)

use crate::token_bucket::TokenBucket;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    Trusted,
    Known,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub burst: u64,
    pub per_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StormPolicy {
    pub global: Rate,
    pub trusted: Rate,
    pub known: Rate,
    pub unknown: Rate,
}

impl Default for StormPolicy {
    fn default() -> Self {
        Self {
            global: Rate {
                burst: 64,
                per_sec: 32,
            },
            trusted: Rate {
                burst: 32,
                per_sec: 16,
            },
            known: Rate {
                burst: 32,
                per_sec: 16,
            },
            // Unknown peers get a deliberately thin slice of the total.
            unknown: Rate {
                burst: 8,
                per_sec: 2,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StormDecision {
    Accept,
    /// Over this class's rate: reject cheaply, before any handshake work.
    RejectClassRate,
    /// Whole-node connection rate exceeded.
    RejectGlobalRate,
}

#[derive(Debug)]
pub struct ConnectionStormGuard {
    global: TokenBucket,
    trusted: TokenBucket,
    known: TokenBucket,
    unknown: TokenBucket,
}

impl ConnectionStormGuard {
    pub fn new(p: StormPolicy, now_millis: u64) -> Self {
        let b = |r: Rate| TokenBucket::new(r.burst, r.per_sec, now_millis);
        Self {
            global: b(p.global),
            trusted: b(p.trusted),
            known: b(p.known),
            unknown: b(p.unknown),
        }
    }

    pub fn admit(&mut self, class: SourceClass, now_millis: u64) -> StormDecision {
        // Check global headroom first so a global rejection doesn't also
        // burn the class's own tokens.
        if self.global.available(now_millis) < 1 {
            return StormDecision::RejectGlobalRate;
        }
        let bucket = match class {
            SourceClass::Trusted => &mut self.trusted,
            SourceClass::Known => &mut self.known,
            SourceClass::Unknown => &mut self.unknown,
        };
        if !bucket.try_consume(1, now_millis) {
            return StormDecision::RejectClassRate;
        }
        let took = self.global.try_consume(1, now_millis);
        debug_assert!(took, "global headroom was checked just above");
        StormDecision::Accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flood_cannot_starve_known_or_trusted() {
        let mut g = ConnectionStormGuard::new(StormPolicy::default(), 0);
        let accepted_unknown = (0..1_000)
            .filter(|_| g.admit(SourceClass::Unknown, 0) == StormDecision::Accept)
            .count();
        assert_eq!(accepted_unknown, 8, "capped at the unknown burst");
        assert_eq!(g.admit(SourceClass::Trusted, 0), StormDecision::Accept);
        assert_eq!(g.admit(SourceClass::Known, 0), StormDecision::Accept);
    }

    #[test]
    fn global_cap_applies_across_classes_without_burning_class_tokens() {
        let mut g = ConnectionStormGuard::new(
            StormPolicy {
                global: Rate {
                    burst: 2,
                    per_sec: 0,
                },
                trusted: Rate {
                    burst: 10,
                    per_sec: 0,
                },
                known: Rate {
                    burst: 10,
                    per_sec: 0,
                },
                unknown: Rate {
                    burst: 10,
                    per_sec: 0,
                },
            },
            0,
        );
        assert_eq!(g.admit(SourceClass::Trusted, 0), StormDecision::Accept);
        assert_eq!(g.admit(SourceClass::Known, 0), StormDecision::Accept);
        assert_eq!(
            g.admit(SourceClass::Trusted, 0),
            StormDecision::RejectGlobalRate
        );
        assert_eq!(
            g.trusted.available(0),
            9,
            "global rejection kept class tokens"
        );
    }

    #[test]
    fn rate_recovers_over_time() {
        let mut g = ConnectionStormGuard::new(StormPolicy::default(), 0);
        for _ in 0..8 {
            g.admit(SourceClass::Unknown, 0);
        }
        assert_eq!(
            g.admit(SourceClass::Unknown, 0),
            StormDecision::RejectClassRate
        );
        assert_eq!(g.admit(SourceClass::Unknown, 1_000), StormDecision::Accept);
    }
}
