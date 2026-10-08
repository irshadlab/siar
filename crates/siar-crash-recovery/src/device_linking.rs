//! §173 "Recovery and QR/NFC Linking" and §174 "Device Link Commit
//! Point".
//!
//! The one rule: a device is linked when — and only when — the
//! certificate/device event is *committed*. The QR/NFC handshake is
//! ephemeral and authorizes nothing by itself, so a crash can never
//! leave a half-authorized device: before the commit it is simply not
//! authorized, after it, it is.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkFacts {
    /// The device certificate / device event is committed to the
    /// durable log.
    pub certificate_committed: bool,
    /// A pre-commit handshake exists and every resumption check
    /// passes (still unexpired, same peer, state machine consistent).
    pub handshake_resumable: bool,
    /// The link attempt's time window has passed.
    pub expired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    Authorized,
    NotAuthorized,
}

/// §174: the commit is the only thing that authorizes.
pub const fn authorization(f: &LinkFacts) -> Authorization {
    if f.certificate_committed {
        Authorization::Authorized
    } else {
        Authorization::NotAuthorized
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRecovery {
    /// Committed before the crash: the device is linked; any leftover
    /// handshake state is just discarded.
    AlreadyLinked,
    /// Not committed, but safely resumable.
    ResumeSafely,
    /// Not committed and not safely resumable (or too old): drop the
    /// attempt and start over.
    ExpireAndRestart,
}

pub const fn recover_link(f: &LinkFacts) -> LinkRecovery {
    if f.certificate_committed {
        LinkRecovery::AlreadyLinked
    } else if f.expired || !f.handshake_resumable {
        LinkRecovery::ExpireAndRestart
    } else {
        LinkRecovery::ResumeSafely
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_facts() -> Vec<LinkFacts> {
        let mut v = Vec::new();
        for c in [false, true] {
            for r in [false, true] {
                for e in [false, true] {
                    v.push(LinkFacts {
                        certificate_committed: c,
                        handshake_resumable: r,
                        expired: e,
                    });
                }
            }
        }
        v
    }

    #[test]
    fn only_the_commit_authorizes_never_the_handshake() {
        for f in all_facts() {
            let expect = if f.certificate_committed {
                Authorization::Authorized
            } else {
                Authorization::NotAuthorized
            };
            assert_eq!(authorization(&f), expect, "{f:?}");
        }
    }

    #[test]
    fn recovery_never_authorizes_without_a_commit() {
        for f in all_facts() {
            if !f.certificate_committed {
                assert_ne!(recover_link(&f), LinkRecovery::AlreadyLinked, "{f:?}");
                assert_eq!(authorization(&f), Authorization::NotAuthorized);
            }
        }
    }

    #[test]
    fn a_committed_link_stays_linked_whatever_the_handshake_state() {
        for f in all_facts().into_iter().filter(|f| f.certificate_committed) {
            assert_eq!(recover_link(&f), LinkRecovery::AlreadyLinked, "{f:?}");
        }
    }

    #[test]
    fn uncommitted_attempts_resume_only_when_fresh_and_resumable() {
        let f = |r, e| LinkFacts {
            certificate_committed: false,
            handshake_resumable: r,
            expired: e,
        };
        assert_eq!(recover_link(&f(true, false)), LinkRecovery::ResumeSafely);
        assert_eq!(recover_link(&f(true, true)), LinkRecovery::ExpireAndRestart);
        assert_eq!(
            recover_link(&f(false, false)),
            LinkRecovery::ExpireAndRestart
        );
        assert_eq!(
            recover_link(&f(false, true)),
            LinkRecovery::ExpireAndRestart
        );
    }

    #[test]
    fn every_case_resolves_to_exactly_one_of_the_two_spec_outcomes_or_linked() {
        for f in all_facts() {
            // the spec's "resume safely or expire/restart" for any
            // uncommitted attempt — no third, half-authorized state
            let r = recover_link(&f);
            assert!(matches!(
                r,
                LinkRecovery::AlreadyLinked
                    | LinkRecovery::ResumeSafely
                    | LinkRecovery::ExpireAndRestart
            ));
        }
    }
}
